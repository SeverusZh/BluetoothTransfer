using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class OppPushServiceTests : IDisposable
{
    private const string TestAddr = "AA:BB:CC:DD:EE:FF";
    private readonly string _tempDir;
    private readonly StorageService _storage;
    private readonly AppConfig _config;

    public OppPushServiceTests()
    {
        _tempDir = Path.Combine(Path.GetTempPath(), "bt_opp_tests_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_tempDir);
        _storage = new StorageService(Path.Combine(_tempDir, "test.db"));
        _config = new AppConfig();
    }

    public void Dispose()
    {
        try
        {
            if (Directory.Exists(_tempDir))
                Directory.Delete(_tempDir, recursive: true);
        }
        catch
        {
            // 清理失败不影响测试结论
        }
    }

    private sealed class FakeOppPushService : OppPushService
    {
        private readonly OppDeviceInfo _device;
        private readonly MemoryObexTransport _transport;

        public FakeOppPushService(EventBus events, StorageService storage, AppConfig config, OppDeviceInfo? device = null)
            : base(events, storage, config)
        {
            _device = device ?? new OppDeviceInfo
            {
                Id = "fake-id",
                Addr = TestAddr,
                Name = "Test Phone",
                IsPaired = true
            };
            _transport = new MemoryObexTransport { AutoReply = true };
        }

        protected override Task<OppDeviceInfo> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
        {
            if (!OppDiscoveryService.AddrEquals(deviceAddr, _device.Addr))
                throw new ObexException($"未找到支持 OPP 的蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）");
            return Task.FromResult(_device);
        }

        protected override Task<IObexTransport> OpenTransportAsync(OppDeviceInfo device, CancellationToken ct)
            => Task.FromResult<IObexTransport>(_transport);
    }

    [Fact]
    public async Task SendFileAsync_Success_WritesOppRecord()
    {
        var filePath = Path.Combine(_tempDir, "hello.txt");
        await File.WriteAllTextAsync(filePath, "hello 蓝牙");
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.True(ok);
        var records = _storage.GetRecords(peerAddr: TestAddr);
        var record = Assert.Single(records);
        Assert.Equal(TransferConst.ChannelOpp, record.Channel);
        Assert.Equal(TransferConst.DirSend, record.Direction);
        Assert.Equal(TransferConst.TypeFile, record.Type);
        Assert.Equal(TransferConst.StatusOk, record.Status);
        Assert.Equal("hello.txt", record.Name);
        Assert.Equal("Test Phone", record.PeerName);
        Assert.Equal(64, record.Checksum.Length);
    }

    [Fact]
    public async Task SendFileAsync_Unpaired_FailsWithNote()
    {
        var filePath = Path.Combine(_tempDir, "a.bin");
        await File.WriteAllBytesAsync(filePath, new byte[] { 1, 2, 3 });
        var unpaired = new OppDeviceInfo { Id = "fake", Addr = TestAddr, Name = "Not Paired", IsPaired = false };
        var push = new FakeOppPushService(new EventBus(), _storage, _config, unpaired);

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.False(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("未配对", record.Note);
    }

    [Fact]
    public async Task SendFileAsync_DeviceNotFound_FailsWithNote()
    {
        var filePath = Path.Combine(_tempDir, "a.bin");
        await File.WriteAllBytesAsync(filePath, new byte[] { 1 });
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        var ok = await push.SendFileAsync("00:00:00:00:00:01", filePath);

        Assert.False(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: "00:00:00:00:00:01"));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("未找到支持 OPP", record.Note);
    }

    [Fact]
    public async Task SendTextAsync_Success_WritesRecordAndCleansTempFile()
    {
        var before = Directory.GetFiles(Path.GetTempPath(), "bt_opp_text_*.txt").Length;
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        var ok = await push.SendTextAsync(TestAddr, "你好，蓝牙");

        Assert.True(ok);
        var after = Directory.GetFiles(Path.GetTempPath(), "bt_opp_text_*.txt").Length;
        Assert.Equal(before, after);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal("bt-note.txt", record.Name);
        Assert.Equal(TransferConst.ChannelOpp, record.Channel);
        Assert.Equal(TransferConst.StatusOk, record.Status);
    }

    [Fact]
    public async Task SendFolderAsync_Success_WritesZipRecordAndCleansTemp()
    {
        var folder = Path.Combine(_tempDir, "docs");
        Directory.CreateDirectory(Path.Combine(folder, "sub"));
        await File.WriteAllTextAsync(Path.Combine(folder, "readme.txt"), "readme");
        await File.WriteAllTextAsync(Path.Combine(folder, "sub", "data.bin"), new string('x', 1024));

        var before = Directory.GetFiles(Path.GetTempPath(), "bt_opp_folder_*.zip").Length;
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        var ok = await push.SendFolderAsync(TestAddr, folder);

        Assert.True(ok);
        var after = Directory.GetFiles(Path.GetTempPath(), "bt_opp_folder_*.zip").Length;
        Assert.Equal(before, after);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal("docs.zip", record.Name);
        Assert.Equal(TransferConst.ChannelOpp, record.Channel);
        Assert.Equal(TransferConst.StatusOk, record.Status);
        Assert.True(record.Size > 0);
    }

    // ---------------------------------------------------------------
    // 重试 / 超时 / 取消 / zip 路径专项测试
    // ---------------------------------------------------------------

    /// <summary>
    /// 假服务：前 <see cref="_failOpenAttempts"/> 次打开传输时抛出可重试的 <see cref="ObexException"/>，
    /// 之后返回可用的内存传输。用于覆盖 OppPushService 的重试逻辑。
    /// </summary>
    private sealed class FailThenSuccessPushService : OppPushService
    {
        private readonly object _lock = new();
        private readonly OppDeviceInfo _device;
        private readonly int _failOpenAttempts;
        private readonly string _errorMessage;
        private int _openCalls;

        public int OpenCalls
        {
            get { lock (_lock) return _openCalls; }
        }

        public FailThenSuccessPushService(EventBus events, StorageService storage, AppConfig config,
            OppDeviceInfo? device = null, int failOpenAttempts = 0, string errorMessage = "连接失败")
            : base(events, storage, config)
        {
            _failOpenAttempts = failOpenAttempts;
            _errorMessage = errorMessage;
            _device = device ?? new OppDeviceInfo
            {
                Id = "fake-id",
                Addr = TestAddr,
                Name = "Test Phone",
                IsPaired = true
            };
        }

        protected override Task<OppDeviceInfo> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
        {
            if (!OppDiscoveryService.AddrEquals(deviceAddr, _device.Addr))
                throw new ObexException($"未找到支持 OPP 的蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）");
            return Task.FromResult(_device);
        }

        protected override Task<IObexTransport> OpenTransportAsync(OppDeviceInfo device, CancellationToken ct)
        {
            bool shouldFail;
            lock (_lock)
            {
                _openCalls++;
                shouldFail = _openCalls <= _failOpenAttempts;
            }
            if (shouldFail) throw new ObexException(_errorMessage);
            return Task.FromResult<IObexTransport>(new MemoryObexTransport { AutoReply = true });
        }
    }

    /// <summary>假服务：每次固定返回同一个传输，便于注入自定义传输验证超时/取消路径。</summary>
    private sealed class TransportPushService : OppPushService
    {
        private readonly OppDeviceInfo _device;
        private readonly IObexTransport _transport;

        public TransportPushService(EventBus events, StorageService storage, AppConfig config,
            IObexTransport transport, OppDeviceInfo? device = null)
            : base(events, storage, config)
        {
            _transport = transport;
            _device = device ?? new OppDeviceInfo
            {
                Id = "fake-id",
                Addr = TestAddr,
                Name = "Test Phone",
                IsPaired = true
            };
        }

        protected override Task<OppDeviceInfo> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
        {
            if (!OppDiscoveryService.AddrEquals(deviceAddr, _device.Addr))
                throw new ObexException($"未找到支持 OPP 的蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）");
            return Task.FromResult(_device);
        }

        protected override Task<IObexTransport> OpenTransportAsync(OppDeviceInfo device, CancellationToken ct)
            => Task.FromResult(_transport);
    }

    /// <summary>
    /// 读取在取消令牌上阻塞的假传输（不需真实蓝牙）：
    /// 首次信号 <see cref="Reading"/> 供测试感知已进入读取，随后 <c>await Task.Delay(Infinite, ct)</c>
    /// 等待调用方取消，用于覆盖“用户取消”记录写入。
    /// </summary>
    private sealed class BlockingReadTransport : IObexTransport
    {
        private readonly TaskCompletionSource _reading = new(TaskCreationOptions.RunContinuationsAsynchronously);

        public Task Reading => _reading.Task;

        public Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
            => Task.CompletedTask;

        public async Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
        {
            _reading.TrySetResult();
            await Task.Delay(Timeout.Infinite, ct);
            return 0;
        }

        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }

    /// <summary>
    /// 读取在取消令牌上阻塞、触发内部 connectCts/sendCts 超时的假传输。
    /// 当 <paramref name="respondToConnectBeforeBlocking"/> 为 true 时，先把 CONNECT OK 响应交给客户端
    /// （连接建立），之后再阻塞以命中 sendCts 超时；否则从一开始就阻塞，命中 connectCts 超时。
    /// </summary>
    private sealed class TimeoutTransport : IObexTransport
    {
        private readonly bool _respondToConnect;
        private byte[]? _connectReply;
        private int _connectPos;

        public TimeoutTransport(bool respondToConnectBeforeBlocking)
        {
            _respondToConnect = respondToConnectBeforeBlocking;
            if (respondToConnectBeforeBlocking)
            {
                _connectReply = new ObexPacket
                {
                    Opcode = ObexResponseCode.Ok,
                    IncludeConnectFields = true,
                    Version = 0x10,
                    Flags = 0x00,
                    MaxPacketLength = ushort.MaxValue
                }.ToBytes();
            }
        }

        public Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
            => Task.CompletedTask;

        public async Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
        {
            // 仍在分发 CONNECT OK 剩余字节（连接已建立），随后开始阻塞以命中 sendCts 超时。
            if (_connectReply != null && _connectPos < _connectReply.Length)
            {
                var n = Math.Min(buffer.Length, _connectReply.Length - _connectPos);
                _connectReply.AsSpan(_connectPos, n).CopyTo(buffer.Span);
                _connectPos += n;
                return n;
            }
            // 阻塞在取消令牌上，等待内部 CancelAfter 触发超时取消。
            await Task.Delay(Timeout.Infinite, ct);
            return 0;
        }

        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }

    /// <summary>
    /// 重试后成功的路径：首次打开传输失败（可重试错误），策略退避后重试成功。
    /// 断言最终成功、尝试次数为 2，且记录为成功。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_Retry_ThenSucceeds()
    {
        var filePath = Path.Combine(_tempDir, "retry.txt");
        await File.WriteAllTextAsync(filePath, "retry 内容");
        // 缩短退避间隔，加快测试；允许 1 次重试（总 2 次尝试）。
        _config.OppRetryCount = 1;
        _config.OppRetryDelaySeconds = 1;
        var push = new FailThenSuccessPushService(new EventBus(), _storage, _config,
            failOpenAttempts: 1, errorMessage: "连接失败：设备未就绪");

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.True(ok);
        Assert.Equal(2, push.OpenCalls); // 第 1 次失败 + 1 次重试成功
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.ChannelOpp, record.Channel);
        Assert.Equal(TransferConst.StatusOk, record.Status);
    }

    /// <summary>
    /// 重试耗尽失败的路径：连续失败且已达最大尝试次数，直接放弃。
    /// 断言失败记录的错误原因包含“已重试 N 次”与原始错误。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_RetryExhausted_WritesFailedWithRetryNote()
    {
        var filePath = Path.Combine(_tempDir, "exhaust.txt");
        await File.WriteAllTextAsync(filePath, "x");
        // 允许 1 次重试（总 2 次尝试），但每次都失败，耗尽后写失败记录。
        _config.OppRetryCount = 1;
        _config.OppRetryDelaySeconds = 1;
        var push = new FailThenSuccessPushService(new EventBus(), _storage, _config,
            failOpenAttempts: 3, errorMessage: "连接失败");

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.False(ok);
        Assert.Equal(2, push.OpenCalls); // 恰好耗尽 2 次尝试
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("连接失败", record.Note);
        Assert.Contains("已重试 1 次", record.Note);
    }

    /// <summary>
    /// 0xC3（对端忙）被判定可重试：连续两次因 0xC3 失败后被重试并最终成功。
    /// 断言最终成功与尝试次数，间接证明该错误分类为可重试。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_Busy0xC3_IsRetriedAndSucceeds()
    {
        var filePath = Path.Combine(_tempDir, "busy.bin");
        await File.WriteAllBytesAsync(filePath, new byte[] { 1, 2, 3 });
        // 允许 2 次重试（总 3 次尝试）。
        _config.OppRetryCount = 2;
        _config.OppRetryDelaySeconds = 1;
        var push = new FailThenSuccessPushService(new EventBus(), _storage, _config,
            failOpenAttempts: 2, errorMessage: "OBEX CONNECT 失败：Forbidden (0xC3)");

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.True(ok);
        Assert.Equal(3, push.OpenCalls); // 2 次 0xC3 失败 + 第 3 次成功
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusOk, record.Status);
    }

    /// <summary>
    /// 取消路径：外部 CancellationToken 在传输读取中取消，服务写入“用户取消”并返回 false。
    /// 通过让假传输在取消令牌上阻塞，不依赖真实蓝牙硬件。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_ExternalCancel_WritesUserCancelRecord()
    {
        var filePath = Path.Combine(_tempDir, "cancel.txt");
        await File.WriteAllTextAsync(filePath, "将被取消");
        var transport = new BlockingReadTransport();
        var push = new TransportPushService(new EventBus(), _storage, _config, transport);
        using var cts = new CancellationTokenSource();

        var task = push.SendFileAsync(TestAddr, filePath, ct: cts.Token);
        await transport.Reading;  // 已进入读取，等待取消
        cts.Cancel();

        var ok = await task;
        Assert.False(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Equal("用户取消", record.Note);
    }

    /// <summary>
    /// connectCts 超时路径：假传输从一开始就在取消令牌上阻塞读取。
    /// 将 OppConnectTimeoutSeconds 配置为 0（服务内部钳制为最小 1 秒），CancelAfter 触发后
    /// 读取抛 OperationCanceledException，服务记录“连接或发送超时”且不重试。
    /// 无需真实蓝牙，也不依赖固定 Sleep——超时由内部 CancelAfter 定时取消驱动。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_ConnectTimeout_WritesTimeoutNote()
    {
        var filePath = Path.Combine(_tempDir, "ctimeout.txt");
        await File.WriteAllTextAsync(filePath, "连接超时");
        _config.OppConnectTimeoutSeconds = 0; // 钳制为 1 秒，最快命中
        _config.OppRetryCount = 0;             // 不重试，直接写超时失败
        var push = new TransportPushService(new EventBus(), _storage, _config, new TimeoutTransport(false));

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.False(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("超时", record.Note);
    }

    /// <summary>
    /// sendCts 超时路径：假传输先派发 CONNECT OK 让连接建立，随后在读取上以 sendCts 取消令牌阻塞，
    /// 命中内部发送超时（OppSendTimeoutSeconds 钳制为 1 秒）。覆盖发送阶段超时记录。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_SendTimeout_WritesTimeoutNote()
    {
        var filePath = Path.Combine(_tempDir, "stimeout.txt");
        await File.WriteAllTextAsync(filePath, "发送超时");
        _config.OppSendTimeoutSeconds = 0; // 钳制为 1 秒
        _config.OppRetryCount = 0;
        var push = new TransportPushService(new EventBus(), _storage, _config, new TimeoutTransport(true));

        var ok = await push.SendFileAsync(TestAddr, filePath);

        Assert.False(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("超时", record.Note);
    }

    /// <summary>
    /// zip:true 路径：SendFileAsync 会把单个文件打入临时 .zip 再推送。
    /// 断言最终成功、记录名为“原名.zip”，且临时 zip 已被清理（与打包前数量一致）。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_ZipTrue_CreatesAndCleansTempZip()
    {
        var filePath = Path.Combine(_tempDir, "data.bin");
        await File.WriteAllBytesAsync(filePath, new byte[1024]);
        var before = Directory.GetFiles(Path.GetTempPath(), "bt_opp_zip_*.zip").Length;

        var push = new FakeOppPushService(new EventBus(), _storage, _config);
        var ok = await push.SendFileAsync(TestAddr, filePath, zip: true);

        Assert.True(ok);
        var after = Directory.GetFiles(Path.GetTempPath(), "bt_opp_zip_*.zip").Length;
        Assert.Equal(before, after); // 临时 zip 已清理
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.ChannelOpp, record.Channel);
        Assert.Equal(TransferConst.StatusOk, record.Status);
        Assert.Equal("data.bin.zip", record.Name);
        Assert.True(record.Size > 0);
    }
}
