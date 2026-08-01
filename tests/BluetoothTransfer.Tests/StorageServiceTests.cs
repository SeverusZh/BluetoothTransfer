using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class StorageServiceTests : IDisposable
{
    private readonly string _dbPath;

    public StorageServiceTests()
    {
        _dbPath = Path.Combine(Path.GetTempPath(), $"bt_test_{Guid.NewGuid():N}.db");
    }

    public void Dispose()
    {
        try
        {
            if (File.Exists(_dbPath)) File.Delete(_dbPath);
        }
        catch
        {
            // 清理失败不影响断言
        }
    }

    private static TransferRecord SampleRecord(string direction = TransferConst.DirSend, string status = TransferConst.StatusOk)
        => new()
        {
            Direction = direction,
            Type = TransferConst.TypeFile,
            PeerName = "测试设备",
            PeerAddr = "00:11:22:33:44:55",
            Name = "test.bin",
            Size = 1024,
            Status = status,
            Channel = TransferConst.ChannelOpp
        };

    [Fact]
    public void ClearRecords_DeletesAllRecords_KeepsDevices()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord(TransferConst.DirRecv, TransferConst.StatusFailed));
        storage.UpsertDevice(new DeviceInfo { Addr = "00:11:22:33:44:55", Name = "测试设备" });

        Assert.Equal(2, storage.GetRecords().Count);
        Assert.Single(storage.GetDevices());

        var deleted = storage.ClearRecords();

        Assert.Equal(2, deleted);
        Assert.Empty(storage.GetRecords());
        Assert.Single(storage.GetDevices()); // 设备表不受影响
    }

    [Fact]
    public void ClearRecords_StatsResetToZero()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord());

        var before = storage.GetStats();
        Assert.Equal(2, before.totalCount);
        Assert.Equal(2, before.sendCount);

        storage.ClearRecords();

        var after = storage.GetStats();
        Assert.Equal(0, after.totalCount);
        Assert.Equal(0, after.totalBytes);
        Assert.Equal(0, after.sendCount);
        Assert.Equal(0, after.recvCount);
    }

    [Fact]
    public void ClearRecords_OnEmptyTable_ReturnsZero()
    {
        var storage = new StorageService(_dbPath);
        Assert.Equal(0, storage.ClearRecords());
    }
}
