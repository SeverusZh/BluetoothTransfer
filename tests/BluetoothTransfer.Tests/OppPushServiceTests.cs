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
}
