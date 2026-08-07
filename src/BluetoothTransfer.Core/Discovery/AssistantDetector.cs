using BluetoothTransfer.Core.Protocol;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Rfcomm;
using System.Text.RegularExpressions;
using Windows.Devices.Enumeration;

namespace BluetoothTransfer.Core.Discovery;

/// <summary>
/// SDP 探测接收端是否运行助手（自定义 UUID）。
/// 输入可以是 OPP 服务条目 Id（Bluetooth#…#RFCOMM:…）或蓝牙设备 Id。
/// </summary>
public static class AssistantDetector
{
    private static readonly Regex RemoteMacRegex = new(
        @"-([0-9A-Fa-f]{2}(?::[0-9A-Fa-f]{2}){5})(?:#|$)", RegexOptions.Compiled);

    /// <summary>已配对蓝牙设备（不依赖 OPP 广播；接收端只运行助手时也能解析）。</summary>
    public sealed record PairedAssistantDevice(string DeviceId, string Addr, string Name);

    /// <summary>按地址在"已配对设备"列表中查找（助手模式发送端的设备解析回退路径）。</summary>
    public static async Task<PairedAssistantDevice?> FindPairedDeviceAsync(string addr, CancellationToken ct = default)
    {
        var target = NormalizeAddr(addr);
        if (string.IsNullOrEmpty(target)) return null;
        var devices = await DeviceInformation.FindAllAsync(
            BluetoothDevice.GetDeviceSelectorFromPairingState(true)).AsTask(ct);
        foreach (var d in devices)
        {
            var mac = ParseMacFromId(d.Id);
            if (string.Equals(NormalizeAddr(mac), target, StringComparison.OrdinalIgnoreCase))
                return new PairedAssistantDevice(d.Id, mac, string.IsNullOrEmpty(d.Name) ? mac : d.Name);
        }
        return null;
    }

    internal static string NormalizeAddr(string addr)
    {
        if (string.IsNullOrEmpty(addr)) return "";
        var clean = addr.Replace(":", "").Replace("-", "").Trim();
        if (clean.Length != 12) return addr;
        return string.Join(":", Enumerable.Range(0, 6).Select(i => clean.Substring(i * 2, 2))).ToLowerInvariant();
    }

    internal static string ParseMacFromId(string id)
    {
        if (string.IsNullOrEmpty(id)) return "";
        var match = RemoteMacRegex.Match(id);
        return match.Success ? match.Groups[1].Value : "";
    }

    public static async Task<RfcommDeviceService?> GetAssistantServiceAsync(
        string deviceId, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(deviceId)) return null;
        var baseId = deviceId;
        var idx = baseId.IndexOf("#RFCOMM", StringComparison.OrdinalIgnoreCase);
        if (idx > 0) baseId = baseId[..idx];

        BluetoothDevice? device = null;
        try
        {
            device = await BluetoothDevice.FromIdAsync(baseId).AsTask(ct);
            if (device == null) return null;
            var result = await device.GetRfcommServicesAsync(BluetoothCacheMode.Uncached).AsTask(ct);
            foreach (var svc in result.Services)
            {
                if (svc.ServiceId.Uuid != AsstConst.ServiceUuid) continue;
                var serviceId = baseId + "#RFCOMM:00000000:{" + AsstConst.ServiceUuid + "}";
                return await RfcommDeviceService.FromIdAsync(serviceId);
            }
            return null;
        }
        catch
        {
            return null;
        }
        finally
        {
            device?.Dispose();
        }
    }

    public static async Task<bool> DeviceHasAssistantAsync(string deviceId, CancellationToken ct = default)
    {
        var service = await GetAssistantServiceAsync(deviceId, ct);
        service?.Dispose();
        return service != null;
    }
}
