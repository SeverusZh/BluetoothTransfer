using System.Diagnostics;
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
        private int _resolveCalls;

        /// <summary>设备解析（含缓存未命中后的真实解析）调用次数，用于验证 DeviceInfoCache 命中。</summary>
        public int ResolveCalls => Volatile.Read(ref _resolveCalls);

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
            Interlocked.Increment(ref _resolveCalls);
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
        // 单文件记录保持 Type=file + 原文件路径，且不写正文。
        Assert.Equal(Path.GetFullPath(filePath), Path.GetFullPath(record.LocalPath));
        Assert.False(record.HasContent);
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
        // 文本记录：Type=text + 正文入库（临时 .txt 已清理，复制只能依赖 content）。
        Assert.Equal(TransferConst.TypeText, record.Type);
        Assert.Equal("你好，蓝牙", record.Content);
        Assert.True(record.HasContent);
        Assert.Contains("已发送", record.Note);
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
        // 文件夹记录：Type=folder + 原始文件夹路径（临时 zip 发送后即删除，仍可用文件夹复制）。
        Assert.Equal(TransferConst.TypeFolder, record.Type);
        Assert.Equal(Path.GetFullPath(folder), Path.GetFullPath(record.LocalPath));
        Assert.False(record.HasContent);
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
        private int _resolveCalls;

        public int OpenCalls
        {
            get { lock (_lock) return _openCalls; }
        }

        /// <summary>设备解析调用次数（含缓存未命中后的真实解析），用于验证失败后失效重试。</summary>
        public int ResolveCalls
        {
            get { lock (_lock) return _resolveCalls; }
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
            lock (_lock) _resolveCalls++;
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
    /// 在每次读取响应前延迟固定时长的内存传输（自动应答 CONNECT/PUT/DISCONNECT）。
    /// 用于验证「整次发送总耗时 &gt; 空闲超时、但每片都有进展」时应成功：
    /// 把 OppSendTimeoutSeconds 配小后，旧实现（整段一次性 CancelAfter）必然失败，新实现应通过。
    /// </summary>
    private sealed class DelayedMemoryTransport : IObexTransport
    {
        private readonly MemoryObexTransport _inner = new() { AutoReply = true };
        private readonly TimeSpan _readDelay;

        public DelayedMemoryTransport(TimeSpan readDelay) => _readDelay = readDelay;

        public Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
            => _inner.WriteAsync(buffer, ct);

        public async Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
        {
            // 每个分片的“写请求 → 等响应”间隔 ≈ _readDelay，远小于空闲超时。
            await Task.Delay(_readDelay, ct);
            return await _inner.ReadAsync(buffer, ct);
        }

        public ValueTask DisposeAsync() => _inner.DisposeAsync();
    }

    /// <summary>
    /// 先正常应答 CONNECT/头包与若干数据分片，随后不再返回任何数据（模拟对端卡死）。
    /// 用于验证空闲超时：有进展时持续成功，卡死后在 OppSendTimeoutSeconds 内取消并写失败记录。
    /// </summary>
    private sealed class StallAfterProgressTransport : IObexTransport
    {
        private readonly Queue<byte[]> _responses = new();
        private readonly int _continueChunks;
        private byte[]? _current;
        private int _currentPos;
        private int _putCount;

        /// <param name="continueChunks">允许成功应答的数据分片数（之后卡死）。</param>
        public StallAfterProgressTransport(int continueChunks) => _continueChunks = continueChunks;

        public Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
        {
            var opcode = buffer.Span[0];
            if (opcode == (byte)ObexRequestOpcode.Connect)
            {
                _responses.Enqueue(new ObexPacket
                {
                    Opcode = ObexResponseCode.Ok,
                    IncludeConnectFields = true,
                    Version = 0x10,
                    Flags = 0x00,
                    MaxPacketLength = ushort.MaxValue
                }.ToBytes());
            }
            else if (opcode == (byte)ObexRequestOpcode.Put)
            {
                _putCount++;
                // 第 1 个 PUT 是头包；其后是数据分片。只对前 N 个分片回 Continue，之后不再回包。
                if (_putCount == 1 || _putCount - 1 <= _continueChunks)
                    _responses.Enqueue(new ObexPacket { Opcode = ObexResponseCode.Continue }.ToBytes());
            }
            else if (opcode == (byte)ObexRequestOpcode.PutFinal)
            {
                _responses.Enqueue(new ObexPacket { Opcode = ObexResponseCode.Ok }.ToBytes());
            }
            else if (opcode == (byte)ObexRequestOpcode.Disconnect)
            {
                _responses.Enqueue(new ObexPacket { Opcode = ObexResponseCode.Ok }.ToBytes());
            }
            return Task.CompletedTask;
        }

        public async Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
        {
            var current = _current;
            if (current == null || _currentPos >= current.Length)
            {
                if (_responses.Count == 0)
                {
                    // 卡死：不再返回数据，等待发送端空闲超时取消令牌。
                    await Task.Delay(Timeout.Infinite, ct);
                    return 0;
                }
                current = _responses.Dequeue();
                _current = current;
                _currentPos = 0;
            }
            // 用局部变量承接当前响应包：await 之后字段的 null 状态不再被编译器信任。
            var n = Math.Min(buffer.Length, current.Length - _currentPos);
            current.AsSpan(_currentPos, n).CopyTo(buffer.Span);
            _currentPos += n;
            return n;
        }

        public ValueTask DisposeAsync() => ValueTask.CompletedTask;
    }

    /// <summary>配对状态可变的假服务：用于验证“未配对失败也会失效设备缓存、下次重新解析”。</summary>
    private sealed class MutableDevicePushService : OppPushService
    {
        private readonly object _lock = new();
        private readonly MemoryObexTransport _transport = new() { AutoReply = true };
        private int _resolveCalls;

        public bool Paired { get; set; }
        public int ResolveCalls
        {
            get { lock (_lock) return _resolveCalls; }
        }

        public MutableDevicePushService(EventBus events, StorageService storage, AppConfig config)
            : base(events, storage, config)
        {
        }

        protected override Task<OppDeviceInfo> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
        {
            lock (_lock) _resolveCalls++;
            return Task.FromResult(new OppDeviceInfo
            {
                Id = "fake-id",
                Addr = TestAddr,
                Name = "Test Phone",
                IsPaired = Paired
            });
        }

        protected override Task<IObexTransport> OpenTransportAsync(OppDeviceInfo device, CancellationToken ct)
            => Task.FromResult<IObexTransport>(_transport);
    }

    /// <summary>
    /// 重试后成功的路径：首次打开传输失败（可重试错误），策略退避后重试成功。
    /// 断言最终成功、尝试次数为 2，且记录为成功；并验证失败后设备缓存失效、重试前重新解析。
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
        Assert.Equal(2, push.ResolveCalls); // 连接失败失效缓存后，重试前重新解析了一次设备
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
        // --zip 仍归为单文件记录：Type=file + 原文件路径（临时 zip 不是复制目标）。
        Assert.Equal(TransferConst.TypeFile, record.Type);
        Assert.Equal(Path.GetFullPath(filePath), Path.GetFullPath(record.LocalPath));
    }

    // ---------------------------------------------------------------
    // 文本记录正文上限 / 设备解析缓存 / 发送空闲超时专项测试
    // ---------------------------------------------------------------

    /// <summary>
    /// 文本正文超过 MaxRecordContentLength：记录仍成功写入，但 content 留空、Note 说明未保存正文，
    /// 复制时由 TransferCopyPlanner 给出中文提示（避免 1MB+ 正文撑大 SQLite 记录）。
    /// </summary>
    [Fact]
    public async Task SendTextAsync_ContentOverLimit_StoresEmptyContentWithNote()
    {
        var text = new string('x', TransferConst.MaxRecordContentLength + 1);
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        var ok = await push.SendTextAsync(TestAddr, text);

        Assert.True(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.TypeText, record.Type);
        Assert.Equal("", record.Content);
        Assert.False(record.HasContent);
        Assert.Contains("未保存正文", record.Note);
        Assert.Contains("上限", record.Note);
        Assert.Equal(TransferConst.MaxRecordContentLength + 1L, record.Size);
    }

    /// <summary>正文上限边界：恰好等于上限保留正文，超过 1 字节则留空并给出说明。</summary>
    [Fact]
    public void PrepareTextRecordContent_AtLimitKeeps_OverLimitOmits()
    {
        var atLimit = new string('x', TransferConst.MaxRecordContentLength);
        var (kept, keptNote) = PushSendHelper.PrepareTextRecordContent(atLimit);
        Assert.Equal(atLimit, kept);
        Assert.Equal("", keptNote);

        var overLimit = new string('x', TransferConst.MaxRecordContentLength + 1);
        var (omitted, omittedNote) = PushSendHelper.PrepareTextRecordContent(overLimit);
        Assert.Equal("", omitted);
        Assert.Contains("未保存正文", omittedNote);
    }

    /// <summary>文本发送失败时，失败记录也应保持 Type=text（与成功记录的类型语义一致）。</summary>
    [Fact]
    public async Task SendTextAsync_DeviceNotFound_WritesTextTypeFailedRecord()
    {
        const string unknownAddr = "00:00:00:00:00:09";
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        var ok = await push.SendTextAsync(unknownAddr, "失败的文本");

        Assert.False(ok);
        var record = Assert.Single(_storage.GetRecords(peerAddr: unknownAddr));
        Assert.Equal(TransferConst.TypeText, record.Type);
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("未找到支持 OPP", record.Note);
    }

    /// <summary>
    /// TTL 内第二次发送同一设备：设备解析应命中 DeviceInfoCache，只解析一次（避免每个文件都全量发现/SDP 枚举）。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_SecondSendWithinTtl_ReusesCachedDeviceResolution()
    {
        var first = Path.Combine(_tempDir, "first.bin");
        var second = Path.Combine(_tempDir, "second.bin");
        await File.WriteAllBytesAsync(first, new byte[] { 1, 2, 3 });
        await File.WriteAllBytesAsync(second, new byte[] { 4, 5, 6 });
        var push = new FakeOppPushService(new EventBus(), _storage, _config);

        Assert.True(await push.SendFileAsync(TestAddr, first));
        Assert.True(await push.SendFileAsync(TestAddr, second));

        Assert.Equal(1, push.ResolveCalls);
        Assert.Equal(2, _storage.GetRecords(peerAddr: TestAddr).Count);
    }

    /// <summary>
    /// 未配对导致失败后应失效设备缓存：用户随后完成配对，下一次发送必须重新解析（而不是继续用“未配对”的旧结论）。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_UnpairedFailure_InvalidatesCacheAndReResolves()
    {
        var filePath = Path.Combine(_tempDir, "pair.bin");
        await File.WriteAllBytesAsync(filePath, new byte[] { 1 });
        var push = new MutableDevicePushService(new EventBus(), _storage, _config) { Paired = false };

        Assert.False(await push.SendFileAsync(TestAddr, filePath));
        Assert.Equal(1, push.ResolveCalls);
        Assert.Equal(1, _storage.GetRecords(peerAddr: TestAddr).Count(r => r.Status == TransferConst.StatusFailed));

        push.Paired = true;
        Assert.True(await push.SendFileAsync(TestAddr, filePath));
        Assert.Equal(2, push.ResolveCalls); // 缓存已失效 → 重新解析
    }

    /// <summary>
    /// 发送空闲超时语义：整次发送总耗时超过 OppSendTimeoutSeconds，但每片间隔远小于它 → 必须成功。
    /// （旧实现把 OppSendTimeoutSeconds 当作整段发送的总超时，此用例必然失败。）
    /// </summary>
    [Fact]
    public async Task SendFileAsync_TotalDurationExceedsIdleTimeout_WithSteadyProgress_Succeeds()
    {
        var filePath = Path.Combine(_tempDir, "slow.bin");
        await File.WriteAllBytesAsync(filePath, new byte[10 * 1024]); // 10 个 1KB 分片
        _config.OppChunkSize = 1024;
        _config.OppSendTimeoutSeconds = 3; // 空闲超时 3 秒；单分片间隔约 400ms（留足时序余量）
        var push = new TransportPushService(new EventBus(), _storage, _config,
            new DelayedMemoryTransport(TimeSpan.FromMilliseconds(400)));

        var sw = Stopwatch.StartNew();
        var ok = await push.SendFileAsync(TestAddr, filePath);
        sw.Stop();

        Assert.True(ok);
        Assert.True(sw.Elapsed > TimeSpan.FromSeconds(3),
            $"总发送耗时 {sw.Elapsed.TotalSeconds:0.00}s 应超过空闲超时 3s，才能证明旧“总超时”语义已被移除");
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusOk, record.Status);
    }

    /// <summary>
    /// 发送空闲超时语义：先有若干分片进展，随后对端卡死不再响应 → 必须在空闲超时后取消并写失败记录
    /// （长时间无进展仍要能及时失败，不能无限等待）。
    /// </summary>
    [Fact]
    public async Task SendFileAsync_StalledAfterProgress_TimesOutByIdleTimeout()
    {
        var filePath = Path.Combine(_tempDir, "stall.bin");
        await File.WriteAllBytesAsync(filePath, new byte[8 * 1024]); // 8 个 1KB 分片，只在第 4 片后卡死
        _config.OppChunkSize = 1024;
        _config.OppSendTimeoutSeconds = 1; // 内部钳制最小 1 秒
        _config.OppRetryCount = 0;
        var push = new TransportPushService(new EventBus(), _storage, _config,
            new StallAfterProgressTransport(continueChunks: 3));

        var sw = Stopwatch.StartNew();
        var ok = await push.SendFileAsync(TestAddr, filePath);
        sw.Stop();

        Assert.False(ok);
        Assert.True(sw.Elapsed >= TimeSpan.FromMilliseconds(900),
            $"卡死后应在约 1s 空闲超时处失败，实际 {sw.Elapsed.TotalSeconds:0.00}s");
        Assert.True(sw.Elapsed < TimeSpan.FromSeconds(20), $"不应长时间挂起，实际 {sw.Elapsed.TotalSeconds:0.00}s");
        var record = Assert.Single(_storage.GetRecords(peerAddr: TestAddr));
        Assert.Equal(TransferConst.StatusFailed, record.Status);
        Assert.Contains("超时", record.Note);
    }
}
