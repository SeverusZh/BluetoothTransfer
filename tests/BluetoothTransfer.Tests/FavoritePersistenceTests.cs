using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class FavoritePersistenceTests : IDisposable
{
    private readonly string _dir;
    private readonly StorageService _storage;

    public FavoritePersistenceTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_fav_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
        _storage = new StorageService(Path.Combine(_dir, "fav.db"));
    }

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch { }
    }

    [Fact]
    public void UpsertFavorite_InsertsRow_ForNeverSeenDevice()
    {
        _storage.UpsertDevice(new DeviceInfo
        {
            Addr = "AA:BB:CC:DD:EE:01",
            Name = "新设备",
            Favorite = true
        });

        var device = Assert.Single(_storage.GetDevices());
        Assert.True(device.Favorite);
        Assert.Equal("新设备", device.Name);
    }

    [Fact]
    public void UpsertWithoutFavorite_PreservesExistingFavorite()
    {
        _storage.UpsertDevice(new DeviceInfo
        {
            Addr = "AA:BB:CC:DD:EE:02",
            Name = "收藏设备",
            Favorite = true
        });

        // 发送成功路径：UpsertDevice 不传 Favorite（默认 false），不应清掉收藏
        _storage.UpsertDevice(new DeviceInfo
        {
            Addr = "AA:BB:CC:DD:EE:02",
            Name = "收藏设备",
            LastSeen = DateTime.Now.ToString("o"),
            LastConnected = DateTime.Now.ToString("o")
        });

        var device = Assert.Single(_storage.GetDevices());
        Assert.True(device.Favorite);
    }

    [Fact]
    public void SetFavoriteFalse_UnsetsExistingFavorite()
    {
        _storage.UpsertDevice(new DeviceInfo
        {
            Addr = "AA:BB:CC:DD:EE:03",
            Name = "设备",
            Favorite = true
        });

        Assert.True(_storage.SetDeviceFavorite("AA:BB:CC:DD:EE:03", false));
        Assert.False(Assert.Single(_storage.GetDevices()).Favorite);
    }
}
