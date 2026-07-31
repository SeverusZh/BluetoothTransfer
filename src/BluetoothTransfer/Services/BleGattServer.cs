using System.Runtime.InteropServices.WindowsRuntime;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Devices.Bluetooth.GenericAttributeProfile;
using Windows.Storage.Streams;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class BleGattServer
{
    private static readonly Guid ServiceUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F60");
    private static readonly Guid TxCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F61");
    private static readonly Guid RxCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F62");
    private static readonly Guid MetaCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F63");

    private readonly EventBus _eventBus;
    private readonly CryptoService _crypto;
    private readonly FrameReassembler _reassembler;
    private GattServiceProvider? _serviceProvider;
    private GattLocalCharacteristic? _rxChar;
    private BluetoothLEAdvertisementPublisher? _publisher;
    private bool _isAdvertising;

    public bool IsAdvertising => _isAdvertising;

    /// <summary>本机名称广播是否生效（仅影响扫描时是否显示名称，不影响 GATT 服务被发现/连接）。</summary>
    public bool IsNameAdvertised
        => _publisher?.Status is BluetoothLEAdvertisementPublisherStatus.Started
            or BluetoothLEAdvertisementPublisherStatus.Waiting;

    public BleGattServer(EventBus eventBus, CryptoService crypto, FrameReassembler reassembler)
    {
        _eventBus = eventBus;
        _crypto = crypto;
        _reassembler = reassembler;
    }

    public async Task<bool> StartAsync(string deviceName)
    {
        try
        {
            var result = await GattServiceProvider.CreateAsync(ServiceUuid);
            if (result.Error != BluetoothError.Success)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"GATT 服务端创建失败：{result.Error}"));
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
                _eventBus.Publish(new LogEvent("ERROR", $"TX 特征创建失败：{txResult.Error}"));
                _serviceProvider.StopAdvertising();
                _serviceProvider = null;
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

            try
            {
                _publisher = new BluetoothLEAdvertisementPublisher();
                _publisher.Advertisement.LocalName = deviceName;
                _publisher.Advertisement.ServiceUuids.Add(ServiceUuid);
                _publisher.Start();
            }
            catch (Exception ex)
            {
                // 部分 Intel 适配器的驱动不支持 BluetoothLEAdvertisementPublisher API。
                // 名称广播只是锦上添花：GATT 服务已通过 StartAdvertising 正常广播，
                // 对端仍可按服务 UUID / 设备地址发现并连接，因此这里只降级不失败。
                _publisher = null;
                _eventBus.Publish(new LogEvent("WARN",
                    $"本机名称广播不可用（{ex.Message}），已降级为仅 GATT 服务广播，" +
                    "对端仍可按服务 UUID / 设备地址发现并连接，但扫描列表不显示本机名称"));
            }

            if (_publisher != null &&
                _publisher.Status != BluetoothLEAdvertisementPublisherStatus.Started &&
                _publisher.Status != BluetoothLEAdvertisementPublisherStatus.Waiting)
            {
                // 发布器进入 Aborted/Stopped 等异常状态：名称广播不可用，但 GATT 广播不受影响，
                // 同样降级处理，避免误报"广播失败"。
                _eventBus.Publish(new LogEvent("WARN",
                    $"本机名称广播状态异常（{_publisher.Status}），已降级为仅 GATT 服务广播"));
                _publisher = null;
            }

            _isAdvertising = true;
            _eventBus.Publish(new LogEvent("INFO", IsNameAdvertised
                ? $"GATT 服务端已启动，广播名称 \"{deviceName}\""
                : $"GATT 服务端已启动（本机名称广播不可用，对端可按服务 UUID / 设备地址连接）"));
            return true;
        }
        catch (Exception ex)
        {
            _serviceProvider?.StopAdvertising();
            _serviceProvider = null;
            _rxChar = null;
            _eventBus.Publish(new LogEvent("ERROR", $"GATT 服务端启动失败：{ex.Message}"));
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
        _eventBus.Publish(new LogEvent("INFO", "GATT 服务端已停止"));
    }

    private async void OnWriteRequested(GattLocalCharacteristic sender, GattWriteRequestedEventArgs args)
    {
        var deferral = args.GetDeferral();
        try
        {
            var request = await args.GetRequestAsync();
            var data = request.Value.ToArray();
            _eventBus.Publish(new LogEvent("DEBUG", $"GATT 收到写入：{data.Length} 字节"));
            HandleReceivedData(data);
            if (request.Option == GattWriteOption.WriteWithResponse)
                request.Respond();
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"GATT 写入处理失败：{ex.Message}"));
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
            _eventBus.Publish(new LogEvent("WARN", "收到无效帧"));
            return;
        }

        switch (frame.MsgType)
        {
            case MsgType.KEY_EXCHANGE:
                HandleKeyExchange(frame.Payload);
                break;
            case MsgType.HEARTBEAT:
                SendAck(frame.TaskId);
                break;
            default:
                _reassembler.HandleFrame(frame, TransferConst.ChannelBle, "", "远程设备");
                SendAck(frame.TaskId);
                break;
        }
    }

    private async void HandleKeyExchange(byte[] peerPublicKey)
    {
        try
        {
            var pub = _crypto.GetPublicKey();
            _crypto.DeriveSessionKey(peerPublicKey);

            if (_rxChar == null)
            {
                _eventBus.Publish(new LogEvent("WARN", "无 RX 特征用于回送公钥"));
                return;
            }

            var frame = new Frame
            {
                MsgType = MsgType.KEY_EXCHANGE,
                TaskId = 0,
                SeqNo = 0,
                TotalLen = (uint)pub.Length,
                Offset = 0,
                Flags = FrameFlags.FinalChunk,
                Payload = pub
            };
            await _rxChar.NotifyValueAsync(frame.Serialize().AsBuffer());
            _eventBus.Publish(new LogEvent("INFO", "会话密钥已建立（服务端）"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"服务端密钥协商失败：{ex.Message}"));
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
            _eventBus.Publish(new LogEvent("ERROR", $"发送 ACK 失败：{ex.Message}"));
        }
    }
}
