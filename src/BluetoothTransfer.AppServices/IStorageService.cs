using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>
/// 存储服务接口：覆盖 GUI 侧（<see cref="OppViewModel"/> 与两个控制器）实际使用的
/// 记录/设备读取写入成员，供单元测试注入 fake 铺路。实现见 <see cref="StorageService"/>。
/// GetRecords 保留全部过滤/limit 可选参数签名，与实现类及调用点一致。
/// </summary>
public interface IStorageService
{
    List<TransferRecord> GetRecords(string? direction = null, string? type = null,
        string? peerAddr = null, string? status = null, DateTime? from = null, DateTime? to = null,
        string? search = null, string orderBy = "created_at DESC", int limit = 200);

    List<DeviceInfo> GetDevices();

    void UpsertDevice(DeviceInfo device);

    bool SetDeviceFavorite(string addr, bool favorite);

    (long totalCount, long totalBytes, long sendCount, long recvCount) GetStats();

    int ClearRecords();
}
