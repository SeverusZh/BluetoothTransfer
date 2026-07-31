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
    private readonly CryptoService _crypto;
    private readonly FrameReassembler _reassembler;
    private BluetoothLEAdvertisementWatcher? _watcher;
    private BluetoothLEDevice? _connectedDevice;
    private GattDeviceService? _service;
    private GattCharacteristic? _txChar;
    private GattCharacteristic? _rxChar;
    private GattCharacteristic? _metaChar;
    private GattSession? _session;
    private readonly Dictionary<ulong, DeviceInfo> _discoveredDevices = new();
    private readonly Dictionary<ulong, (DateTime Time, int Rssi)> _lastPublish = new();
    private static readonly TimeSpan MinPublishInterval = TimeSpan.FromSeconds(1);
    private const int MinRssiDelta = 3;

    public bool IsScanning => _watcher?.Status == BluetoothLEAdvertisementWatcherStatus.Started;
    public bool IsConnected => _connectedDevice != null;
    public string? ConnectedAddr { get; private set; }
    public string? ConnectedName { get; private set; }

    public BleService(EventBus eventBus, CryptoService crypto, FrameReassembler reassembler)
    {
        _eventBus = eventBus;
        _crypto = crypto;
        _reassembler = reassembler;
    }

    public void StartScan()
    {
        if (IsScanning) return;
        _discoveredDevices.Clear();
        _lastPublish.Clear();
        _watcher = new BluetoothLEAdvertisementWatcher
        {
            ScanningMode = BluetoothLEScanningMode.Active
        };
        _watcher.Received += OnAdvertisementReceived;
        _watcher.Start();
        _eventBus.Publish(new LogEvent("INFO", "BLE 扫描已启动"));
    }

    public void StopScan()
    {
        if (_watcher == null) return;
        _watcher.Received -= OnAdvertisementReceived;
        _watcher.Stop();
        _watcher = null;
        _eventBus.Publish(new LogEvent("INFO", "BLE 扫描已停止"));
    }

    private void OnAdvertisementReceived(BluetoothLEAdvertisementWatcher sender, BluetoothLEAdvertisementReceivedEventArgs args)
    {
        var addr = args.BluetoothAddress.ToString("X12");
        var name = args.Advertisement.LocalName;
        if (string.IsNullOrEmpty(name)) return;

        var rssi = args.RawSignalStrengthInDBm;
        var now = DateTime.UtcNow;

        if (_discoveredDevices.TryGetValue(args.BluetoothAddress, out var existing))
        {
            // 已发现的设备也要持续发布事件（携带最新 RSSI / 时间戳），
            // 否则 UI 中的信号强度停留在首次发现值，永不刷新。
            existing.Rssi = rssi;
            existing.LastSeen = DateTime.Now.ToString("o");
        }
        else
        {
            existing = new DeviceInfo
            {
                Addr = addr,
                Name = name,
                Rssi = rssi,
                LastSeen = DateTime.Now.ToString("o")
            };
            _discoveredDevices[args.BluetoothAddress] = existing;
        }

        // 节流：BLE 设备通常每秒广播 1-10 次，若每次都发布事件，
        // 扫描期间会持续触发 Dispatcher.Invoke 造成 UI 卡顿。
        // 仅当信号强度显著变化（>=3dBm）或距上次发布超过 1 秒时才发布。
        var throttled = _lastPublish.TryGetValue(args.BluetoothAddress, out var last)
            && now - last.Time < MinPublishInterval
            && Math.Abs(rssi - last.Rssi) < MinRssiDelta;
        if (throttled) return;

        _lastPublish[args.BluetoothAddress] = (now, rssi);
        _eventBus.Publish(new DeviceDiscoveredEvent(addr, name, rssi));
    }

    public async Task<bool> ConnectAsync(string addr)
    {
        BluetoothLEDevice? bleDevice = null;
        try
        {
            _eventBus.Publish(new LogEvent("INFO", $"正在连接 {addr}..."));
            var selector = $"System.Devices.Aep.ProtocolId:=\"{{bb7bb05e-5972-42b5-94fc-76eaa7084d49}}\" AND System.Devices.Aep.DeviceAddress:=\"{FormatMac(addr)}\"";
            var devices = await DeviceInformation.FindAllAsync(selector);

            if (devices.Count > 0)
                bleDevice = await BluetoothLEDevice.FromIdAsync(devices[0].Id);
            else
                bleDevice = await BluetoothLEDevice.FromBluetoothAddressAsync(ParseMac(addr));

            if (bleDevice == null)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"未找到设备 {addr}"));
                return false;
            }

            var services = await bleDevice.GetGattServicesForUuidAsync(ServiceUuid);
            if (services.Status != GattCommunicationStatus.Success || services.Services.Count == 0)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"设备 {addr} 上未找到 GATT 服务"));
                bleDevice.Dispose();
                return false;
            }

            _service = services.Services[0];
            var txChars = await _service.GetCharacteristicsForUuidAsync(TxCharUuid);
            var rxChars = await _service.GetCharacteristicsForUuidAsync(RxCharUuid);
            var metaChars = await _service.GetCharacteristicsForUuidAsync(MetaCharUuid);

            if (txChars.Status != GattCommunicationStatus.Success || txChars.Characteristics.Count == 0)
            {
                _eventBus.Publish(new LogEvent("ERROR", "未找到 TX 特征"));
                _service.Dispose();
                _service = null;
                bleDevice.Dispose();
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
            try
            {
                var deviceId = BluetoothDeviceId.FromId(bleDevice.DeviceId);
                _session = await GattSession.FromDeviceIdAsync(deviceId);
                if (_session != null)
                {
                    _session.MaintainConnection = true;
                    _session.MaxPduSizeChanged += OnMaxPduSizeChanged;
                }
            }
            catch
            {
                // 会话建立失败时退回最小 MTU 估算，不影响连接本身。
            }

            _eventBus.Publish(new DeviceConnectedEvent(addr, bleDevice.Name));
            _eventBus.Publish(new LogEvent("INFO", $"已连接到 {bleDevice.Name}（{addr}）"));

            await InitiateKeyExchangeAsync();
            return true;
        }
        catch (Exception ex)
        {
            if (_connectedDevice == null)
            {
                _service?.Dispose();
                _service = null;
                bleDevice?.Dispose();
            }
            _eventBus.Publish(new LogEvent("ERROR", $"连接失败：{ex.Message}"));
            return false;
        }
    }

    public void Disconnect()
    {
        CleanupConnection();
        if (ConnectedAddr != null)
        {
            _eventBus.Publish(new DeviceDisconnectedEvent(ConnectedAddr));
            _eventBus.Publish(new LogEvent("INFO", $"已断开 {ConnectedAddr}"));
        }
        ConnectedAddr = null;
        ConnectedName = null;
    }

    private void CleanupConnection()
    {
        if (_rxChar != null)
        {
            _rxChar.ValueChanged -= OnRxValueChanged;
            _rxChar = null;
        }
        _txChar = null;
        _metaChar = null;
        if (_session != null)
        {
            _session.MaxPduSizeChanged -= OnMaxPduSizeChanged;
            _session.Dispose();
            _session = null;
        }
        _service?.Dispose();
        _service = null;
        if (_connectedDevice != null)
        {
            _connectedDevice.ConnectionStatusChanged -= OnConnectionStatusChanged;
            _connectedDevice.Dispose();
            _connectedDevice = null;
        }
    }

    private void OnConnectionStatusChanged(BluetoothLEDevice sender, object args)
    {
        if (sender.ConnectionStatus != BluetoothConnectionStatus.Disconnected) return;

        var addr = ConnectedAddr ?? "";
        CleanupConnection();
        ConnectedAddr = null;
        ConnectedName = null;
        _eventBus.Publish(new DeviceDisconnectedEvent(addr));
        _eventBus.Publish(new LogEvent("WARN", $"设备 {addr} 已断开"));
    }

    private void OnRxValueChanged(GattCharacteristic sender, GattValueChangedEventArgs args)
    {
        try
        {
            var data = args.CharacteristicValue.ToArray();
            var frame = Frame.Deserialize(data);
            if (frame == null) return;

            switch (frame.MsgType)
            {
                case MsgType.KEY_EXCHANGE:
                    HandlePeerPublicKey(frame.Payload);
                    break;
                case MsgType.ACK:
                    _eventBus.Publish(new LogEvent("DEBUG", $"收到任务 {frame.TaskId} 的 ACK"));
                    break;
                default:
                    _reassembler.HandleFrame(frame, TransferConst.ChannelBle, ConnectedAddr ?? "", ConnectedName ?? "");
                    break;
            }
        }
        catch (Exception ex)
        {
            // GATT 回调在系统线程上执行，未捕获异常可能导致应用崩溃，必须兜底。
            _eventBus.Publish(new LogEvent("ERROR", $"接收 GATT 数据异常：{ex.Message}"));
        }
    }

    private async Task InitiateKeyExchangeAsync()
    {
        try
        {
            if (_txChar == null) return;
            var pub = _crypto.GetPublicKey();
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
            var result = await _txChar.WriteValueAsync(frame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
            if (result == GattCommunicationStatus.Success)
                _eventBus.Publish(new LogEvent("INFO", "密钥协商已发起"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"密钥协商发起失败：{ex.Message}"));
        }
    }

    private void HandlePeerPublicKey(byte[] payload)
    {
        try
        {
            _crypto.DeriveSessionKey(payload);
            _eventBus.Publish(new LogEvent("INFO", "会话密钥已建立"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"密钥派生失败：{ex.Message}"));
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
            _eventBus.Publish(new LogEvent("ERROR", $"发送文本失败：{ex.Message}"));
            return false;
        }
    }

    public async Task<bool> SendBinaryChunkedAsync(MsgType msgType, uint taskId, byte[] data, FrameFlags flags, bool encrypt = false)
    {
        if (_txChar == null) return false;
        if (encrypt && !_crypto.HasSessionKey)
        {
            _eventBus.Publish(new LogEvent("WARN", "请求加密但无会话密钥，将以明文发送"));
            encrypt = false;
        }
        try
        {
            const int cryptoOverhead = 28;
            var maxPayload = GetMaxPayloadSize();
            var plainChunk = encrypt ? Math.Max(maxPayload - cryptoOverhead, 1) : maxPayload;
            var totalLen = (uint)data.Length;

            if (totalLen == 0)
            {
                // 空文件也必须发送一个 FinalChunk 分片，
                // 否则接收端重组器永远等不到收尾，文件静默丢失。
                var emptyFrame = new Frame
                {
                    MsgType = msgType,
                    TaskId = taskId,
                    SeqNo = 0,
                    TotalLen = 0,
                    Offset = 0,
                    Flags = flags | FrameFlags.FinalChunk,
                    Payload = Array.Empty<byte>()
                };
                var emptyResult = await _txChar.WriteValueAsync(emptyFrame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
                return emptyResult == GattCommunicationStatus.Success;
            }

            ushort seq = 0;
            uint offset = 0;

            while (offset < totalLen)
            {
                var len = (int)Math.Min(plainChunk, totalLen - offset);
                var chunk = new byte[len];
                Array.Copy(data, offset, chunk, 0, len);
                var isFinal = offset + len >= totalLen;

                byte[] payload = chunk;
                var frameFlags = flags;
                if (encrypt)
                {
                    payload = _crypto.Encrypt(chunk);
                    frameFlags |= FrameFlags.Encrypted;
                }
                if (isFinal)
                    frameFlags |= FrameFlags.FinalChunk;

                var frame = new Frame
                {
                    MsgType = msgType,
                    TaskId = taskId,
                    SeqNo = seq++,
                    TotalLen = totalLen,
                    Offset = offset,
                    Flags = frameFlags,
                    Payload = payload
                };
                var result = await _txChar.WriteValueAsync(frame.Serialize().AsBuffer(), GattWriteOption.WriteWithResponse);
                if (result != GattCommunicationStatus.Success) return false;
                offset += (uint)len;
            }
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"发送二进制数据失败：{ex.Message}"));
            return false;
        }
    }

    private int GetMaxPayloadSize()
    {
        const int frameOverhead = 21;
        const int attHeaderOverhead = 3;

        // 协商完成前按最小 ATT MTU 保守估算（attribute value 上限 20 字节），
        // 避免按固定 180 组出超过对端实际可接收长度的写请求；
        // MTU 协商完成后经 MaxPduSizeChanged 以实际值重新计算。
        var attrValueMax = 20;
        try
        {
            if (_session != null && _session.MaxPduSize > 23)
            {
                // MaxPduSize 在不同文档中语义略有差异（ATT MTU 或 value 上限），
                // 保守再减 3 字节 ATT 头，确保组帧后的写请求不会超过对端可接收长度。
                attrValueMax = Math.Max((int)_session.MaxPduSize - attHeaderOverhead, 20);
            }
        }
        catch
        {
            // 读取失败时沿用兜底值。
        }

        return Math.Max(attrValueMax - frameOverhead, 20);
    }

    private void OnMaxPduSizeChanged(GattSession session, object args)
    {
        try
        {
            var pdu = session.MaxPduSize;
            _eventBus.Publish(new LogEvent("INFO", $"BLE MTU 协商完成：MaxPduSize={pdu}"));
            if (pdu <= 23)
                _eventBus.Publish(new LogEvent("WARN", "对端未启用扩展 MTU，分帧传输可能因帧头开销过大而失败"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"读取 MTU 协商结果失败：{ex.Message}"));
        }
    }

    public async Task<bool> SendMetaAsync(byte[] metaPayload, uint taskId)
    {
        var target = _metaChar ?? _txChar;
        if (target == null) return false;
        try
        {
            var frame = new Frame
            {
                MsgType = MsgType.META,
                TaskId = taskId,
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
            _eventBus.Publish(new LogEvent("ERROR", $"发送元数据失败：{ex.Message}"));
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
