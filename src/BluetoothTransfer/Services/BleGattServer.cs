using System.Runtime.InteropServices.WindowsRuntime;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Devices.Bluetooth.GenericAttributeProfile;
using Windows.Storage.Streams;

namespace BluetoothTransfer.Services;

public class BleGattServer
{
    private static readonly Guid ServiceUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F60");
    private static readonly Guid TxCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F61");
    private static readonly Guid RxCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F62");
    private static readonly Guid MetaCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F63");

    private readonly EventBus _eventBus;
    private GattServiceProvider? _serviceProvider;
    private GattLocalCharacteristic? _rxChar;
    private BluetoothLEAdvertisementPublisher? _publisher;
    private bool _isAdvertising;

    public bool IsAdvertising => _isAdvertising;

    public BleGattServer(EventBus eventBus)
    {
        _eventBus = eventBus;
    }

    public async Task<bool> StartAsync(string deviceName)
    {
        try
        {
            var result = await GattServiceProvider.CreateAsync(ServiceUuid);
            if (result.Error != BluetoothError.Success)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"GATT server create failed: {result.Error}"));
                return false;
            }

            _serviceProvider = result.ServiceProvider;

            var txResult = await _serviceProvider.Service.CreateCharacteristicAsync(TxCharUuid,
                new GattLocalCharacteristicParameters
                {
                    CharacteristicProperties = GattCharacteristicProperties.Write | GattCharacteristicProperties.WriteWithoutResponse,
                    WriteProtectionLevel = GattProtectionLevel.Plain
                });

            if (txResult.Error != BluetoothError.Success)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"TX char create failed: {txResult.Error}"));
                return false;
            }
            txResult.Characteristic.WriteRequested += OnWriteRequested;

            var rxResult = await _serviceProvider.Service.CreateCharacteristicAsync(RxCharUuid,
                new GattLocalCharacteristicParameters
                {
                    CharacteristicProperties = GattCharacteristicProperties.Notify,
                    WriteProtectionLevel = GattProtectionLevel.Plain
                });
            if (rxResult.Error == BluetoothError.Success)
                _rxChar = rxResult.Characteristic;

            var metaResult = await _serviceProvider.Service.CreateCharacteristicAsync(MetaCharUuid,
                new GattLocalCharacteristicParameters
                {
                    CharacteristicProperties = GattCharacteristicProperties.Write | GattCharacteristicProperties.WriteWithoutResponse,
                    WriteProtectionLevel = GattProtectionLevel.Plain
                });
            if (metaResult.Error == BluetoothError.Success)
                metaResult.Characteristic.WriteRequested += OnWriteRequested;

            _serviceProvider.StartAdvertising(new GattServiceProviderAdvertisingParameters
            {
                IsConnectable = true,
                IsDiscoverable = true
            });

            _publisher = new BluetoothLEAdvertisementPublisher();
            _publisher.Advertisement.LocalName = deviceName;
            _publisher.Advertisement.ServiceUuids.Add(ServiceUuid);
            _publisher.Start();

            _isAdvertising = true;
            _eventBus.Publish(new LogEvent("INFO", $"GATT server started, advertising as \"{deviceName}\""));
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"GATT server start failed: {ex.Message}"));
            return false;
        }
    }

    public void Stop()
    {
        _publisher?.Stop();
        _publisher = null;
        _serviceProvider?.StopAdvertising();
        _serviceProvider = null;
        _rxChar = null;
        _isAdvertising = false;
        _eventBus.Publish(new LogEvent("INFO", "GATT server stopped"));
    }

    private async void OnWriteRequested(GattLocalCharacteristic sender, GattWriteRequestedEventArgs args)
    {
        var deferral = args.GetDeferral();
        try
        {
            var request = await args.GetRequestAsync();
            var data = request.Value.ToArray();
            _eventBus.Publish(new LogEvent("DEBUG", $"GATT write received: {data.Length} bytes"));
            HandleReceivedData(data);
            if (request.Option == GattWriteOption.WriteWithResponse)
                request.Respond();
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"GATT write handling failed: {ex.Message}"));
        }
        finally
        {
            deferral.Complete();
        }
    }

    public void HandleReceivedData(byte[] data)
    {
        var frame = Frame.Deserialize(data);
        if (frame == null)
        {
            _eventBus.Publish(new LogEvent("WARN", "Invalid frame received"));
            return;
        }

        switch (frame.MsgType)
        {
            case MsgType.DATA:
                var text = System.Text.Encoding.UTF8.GetString(frame.Payload);
                _eventBus.Publish(new TextReceivedEvent("remote", "Remote Device", text));
                SendAck(frame.TaskId);
                break;
            case MsgType.META:
                var meta = MetaPayload.Decode(frame.Payload);
                if (meta != null)
                    _eventBus.Publish(new LogEvent("INFO", $"META received: {meta.Value.type} / {meta.Value.name} / {meta.Value.size} bytes"));
                SendAck(frame.TaskId);
                break;
            case MsgType.HEARTBEAT:
                SendAck(frame.TaskId);
                break;
        }
    }

    private async void SendAck(uint taskId)
    {
        if (_rxChar == null) return;
        try
        {
            var ackFrame = new Frame
            {
                MsgType = MsgType.ACK,
                TaskId = taskId,
                SeqNo = 0,
                TotalLen = 0,
                Offset = 0,
                Flags = FrameFlags.None,
                Payload = Array.Empty<byte>()
            };
            var buffer = ackFrame.Serialize().AsBuffer();
            await _rxChar.NotifyValueAsync(buffer);
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Send ACK failed: {ex.Message}"));
        }
    }
}
