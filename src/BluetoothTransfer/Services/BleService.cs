using System.Runtime.InteropServices.WindowsRuntime;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Advertisement;
using Windows.Devices.Bluetooth.GenericAttributeProfile;
using Windows.Devices.Enumeration;
using Windows.Storage.Streams;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class BleService
{
    private static readonly Guid ServiceUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F60");
    private static readonly Guid TxCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F61");
    private static readonly Guid RxCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F62");
    private static readonly Guid MetaCharUuid = Guid.Parse("A0E9F1D2-7B3C-4E5A-9F8D-1C2B3A4E5F63");

    private readonly EventBus _eventBus;
    private BluetoothLEAdvertisementWatcher? _watcher;
    private BluetoothLEDevice? _connectedDevice;
    private GattDeviceService? _service;
    private GattCharacteristic? _txChar;
    private GattCharacteristic? _rxChar;
    private GattCharacteristic? _metaChar;
    private readonly Dictionary<ulong, DeviceInfo> _discoveredDevices = new();

    public bool IsScanning => _watcher?.Status == BluetoothLEAdvertisementWatcherStatus.Started;
    public bool IsConnected => _connectedDevice != null;
    public string? ConnectedAddr { get; private set; }
    public string? ConnectedName { get; private set; }

    public BleService(EventBus eventBus)
    {
        _eventBus = eventBus;
    }

    public void StartScan()
    {
        if (IsScanning) return;
        _discoveredDevices.Clear();
        _watcher = new BluetoothLEAdvertisementWatcher
        {
            ScanningMode = BluetoothLEScanningMode.Active
        };
        _watcher.Received += OnAdvertisementReceived;
        _watcher.Start();
        _eventBus.Publish(new LogEvent("INFO", "BLE scan started"));
    }

    public void StopScan()
    {
        if (_watcher == null) return;
        _watcher.Received -= OnAdvertisementReceived;
        _watcher.Stop();
        _watcher = null;
        _eventBus.Publish(new LogEvent("INFO", "BLE scan stopped"));
    }

    private void OnAdvertisementReceived(BluetoothLEAdvertisementWatcher sender, BluetoothLEAdvertisementReceivedEventArgs args)
    {
        var addr = args.BluetoothAddress.ToString("X12");
        var name = args.Advertisement.LocalName;
        if (string.IsNullOrEmpty(name)) return;

        if (!_discoveredDevices.ContainsKey(args.BluetoothAddress))
        {
            var device = new DeviceInfo
            {
                Addr = addr,
                Name = name,
                Rssi = args.RawSignalStrengthInDBm,
                LastSeen = DateTime.Now.ToString("o")
            };
            _discoveredDevices[args.BluetoothAddress] = device;
            _eventBus.Publish(new DeviceDiscoveredEvent(addr, name, args.RawSignalStrengthInDBm));
        }
    }

    public async Task<bool> ConnectAsync(string addr)
    {
        try
        {
            _eventBus.Publish(new LogEvent("INFO", $"Connecting to {addr}..."));
            var selector = $"System.Devices.Aep.ProtocolId:=\"{{bb7bb05e-5972-42b5-94fc-76eaa7084d49}}\" AND System.Devices.Aep.DeviceAddress:=\"{FormatMac(addr)}\"";
            var devices = await DeviceInformation.FindAllAsync(selector);

            BluetoothLEDevice? bleDevice = null;
            if (devices.Count > 0)
            {
                bleDevice = await BluetoothLEDevice.FromIdAsync(devices[0].Id);
            }
            else
            {
                bleDevice = await BluetoothLEDevice.FromBluetoothAddressAsync(ParseMac(addr));
            }

            if (bleDevice == null)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"Device {addr} not found"));
                return false;
            }

            var services = await bleDevice.GetGattServicesForUuidAsync(ServiceUuid);
            if (services.Status != GattCommunicationStatus.Success || services.Services.Count == 0)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"GATT service not found on {addr}"));
                bleDevice.Dispose();
                return false;
            }

            _service = services.Services[0];
            var txChars = await _service.GetCharacteristicsForUuidAsync(TxCharUuid);
            var rxChars = await _service.GetCharacteristicsForUuidAsync(RxCharUuid);
            var metaChars = await _service.GetCharacteristicsForUuidAsync(MetaCharUuid);

            if (txChars.Status != GattCommunicationStatus.Success || txChars.Characteristics.Count == 0)
            {
                _eventBus.Publish(new LogEvent("ERROR", "TX characteristic not found"));
                return false;
            }

            _txChar = txChars.Characteristics[0];
            _metaChar = metaChars.Status == GattCommunicationStatus.Success && metaChars.Characteristics.Count > 0
                ? metaChars.Characteristics[0] : null;

            if (rxChars.Status == GattCommunicationStatus.Success && rxChars.Characteristics.Count > 0)
            {
                _rxChar = rxChars.Characteristics[0];
                await _rxChar.WriteClientCharacteristicConfigurationDescriptorAsync(
                    GattClientCharacteristicConfigurationDescriptorValue.Notify);
                _rxChar.ValueChanged += OnRxValueChanged;
            }

            _connectedDevice = bleDevice;
            ConnectedAddr = addr;
            ConnectedName = bleDevice.Name;
            _connectedDevice.ConnectionStatusChanged += OnConnectionStatusChanged;

            _eventBus.Publish(new DeviceConnectedEvent(addr, bleDevice.Name));
            _eventBus.Publish(new LogEvent("INFO", $"Connected to {bleDevice.Name} ({addr})"));
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Connect failed: {ex.Message}"));
            return false;
        }
    }

    public void Disconnect()
    {
        if (_rxChar != null)
        {
            _rxChar.ValueChanged -= OnRxValueChanged;
            _rxChar = null;
        }
        _txChar = null;
        _metaChar = null;
        _service?.Dispose();
        _service = null;
        _connectedDevice?.Dispose();
        _connectedDevice = null;

        if (ConnectedAddr != null)
        {
            _eventBus.Publish(new DeviceDisconnectedEvent(ConnectedAddr));
            _eventBus.Publish(new LogEvent("INFO", $"Disconnected from {ConnectedAddr}"));
        }
        ConnectedAddr = null;
        ConnectedName = null;
    }

    private void OnConnectionStatusChanged(BluetoothLEDevice sender, object args)
    {
        if (sender.ConnectionStatus == BluetoothConnectionStatus.Disconnected)
        {
            _eventBus.Publish(new DeviceDisconnectedEvent(ConnectedAddr ?? ""));
            _eventBus.Publish(new LogEvent("WARN", $"Device {ConnectedAddr} disconnected"));
        }
    }

    private void OnRxValueChanged(GattCharacteristic sender, GattValueChangedEventArgs args)
    {
        var data = args.CharacteristicValue.ToArray();
        var frame = Frame.Deserialize(data);
        if (frame == null) return;

        switch (frame.MsgType)
        {
            case MsgType.DATA:
                var text = System.Text.Encoding.UTF8.GetString(frame.Payload);
                _eventBus.Publish(new TextReceivedEvent(ConnectedAddr ?? "", ConnectedName ?? "", text));
                break;
            case MsgType.ACK:
                _eventBus.Publish(new LogEvent("DEBUG", $"ACK received for task {frame.TaskId}"));
                break;
        }
    }

    public async Task<bool> SendTextAsync(string text)
    {
        if (_txChar == null) return false;
        try
        {
            var payload = System.Text.Encoding.UTF8.GetBytes(text);
            var taskId = (uint)Random.Shared.Next();
            var totalLen = (uint)payload.Length;
            var maxPayload = GetMaxPayloadSize();

            if (payload.Length <= maxPayload)
            {
                var frame = new Frame
                {
                    MsgType = MsgType.DATA,
                    TaskId = taskId,
                    SeqNo = 0,
                    TotalLen = totalLen,
                    Offset = 0,
                    Flags = FrameFlags.FinalChunk,
                    Payload = payload
                };
                var result = await _txChar.WriteValueAsync(frame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
                return result == GattCommunicationStatus.Success;
            }

            ushort seq = 0;
            uint offset = 0;
            while (offset < totalLen)
            {
                var len = (int)Math.Min(maxPayload, totalLen - offset);
                var chunk = new byte[len];
                Array.Copy(payload, offset, chunk, 0, len);
                var isFinal = offset + len >= totalLen;

                var frame = new Frame
                {
                    MsgType = MsgType.DATA,
                    TaskId = taskId,
                    SeqNo = seq++,
                    TotalLen = totalLen,
                    Offset = offset,
                    Flags = isFinal ? FrameFlags.FinalChunk : FrameFlags.None,
                    Payload = chunk
                };
                var result = await _txChar.WriteValueAsync(frame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
                if (result != GattCommunicationStatus.Success) return false;
                offset += (uint)len;
            }
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Send text failed: {ex.Message}"));
            return false;
        }
    }

    public async Task<bool> SendBinaryChunkedAsync(MsgType msgType, uint taskId, byte[] data, FrameFlags flags)
    {
        if (_txChar == null) return false;
        try
        {
            var maxPayload = GetMaxPayloadSize();
            var totalLen = (uint)data.Length;
            ushort seq = 0;
            uint offset = 0;

            while (offset < totalLen)
            {
                var len = (int)Math.Min(maxPayload, totalLen - offset);
                var chunk = new byte[len];
                Array.Copy(data, offset, chunk, 0, len);
                var isFinal = offset + len >= totalLen;

                var frame = new Frame
                {
                    MsgType = msgType,
                    TaskId = taskId,
                    SeqNo = seq++,
                    TotalLen = totalLen,
                    Offset = offset,
                    Flags = isFinal ? (flags | FrameFlags.FinalChunk) : flags,
                    Payload = chunk
                };
                var result = await _txChar.WriteValueAsync(frame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
                if (result != GattCommunicationStatus.Success) return false;
                offset += (uint)len;
            }
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Send binary failed: {ex.Message}"));
            return false;
        }
    }

    private int GetMaxPayloadSize()
    {
        const int frameOverhead = 21;
        const int mtu = 180;
        return Math.Max(mtu - frameOverhead, 20);
    }

    public async Task<bool> SendMetaAsync(byte[] metaPayload)
    {
        var target = _metaChar ?? _txChar;
        if (target == null) return false;
        try
        {
            var frame = new Frame
            {
                MsgType = MsgType.META,
                TaskId = (uint)Random.Shared.Next(),
                SeqNo = 0,
                TotalLen = (uint)metaPayload.Length,
                Offset = 0,
                Flags = FrameFlags.FinalChunk,
                Payload = metaPayload
            };
            var result = await target.WriteValueAsync(frame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
            return result == GattCommunicationStatus.Success;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Send meta failed: {ex.Message}"));
            return false;
        }
    }

    private static string FormatMac(string addr)
    {
        var clean = addr.Replace(":", "").Replace("-", "");
        return string.Join(":", Enumerable.Range(0, 6).Select(i => clean.Substring(i * 2, 2)));
    }

    private static ulong ParseMac(string addr)
    {
        var clean = addr.Replace(":", "").Replace("-", "");
        return ulong.Parse(clean, System.Globalization.NumberStyles.HexNumber);
    }
}
