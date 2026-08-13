namespace BluetoothTransfer.Services;

/// <summary>
/// OPP 设备发现与配对的实例成员接口：覆盖 GUI 侧实际使用的 DiscoverAsync/PairAsync。
/// 静态辅助（AddrEquals/NormalizeAddr/OpenBluetoothSettings）留在 <see cref="OppDiscoveryService"/> 类上，
/// 不进接口，供实现处直接调用。实现见 <see cref="OppDiscoveryService"/>。
/// </summary>
public interface IOppDiscoveryService
{
    Task<List<OppDeviceInfo>> DiscoverAsync(bool pairedOnly = false, int seconds = 0, CancellationToken ct = default);

    Task<bool> PairAsync(string addr, string? pin = null, CancellationToken ct = default);
}
