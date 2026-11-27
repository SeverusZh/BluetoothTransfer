using BluetoothTransfer.Core.Protocol;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Rfcomm;
using System.Text.RegularExpressions;
using Windows.Devices.Enumeration;

namespace BluetoothTransfer.Core.Discovery;

/// <summary>
/// SDP/AEP 探测接收端是否运行助手（自定义 UUID）。
/// 输入可以是 OPP 服务条目 Id（Bluetooth#…#RFCOMM:…）或蓝牙设备 Id。
///
/// 探测结论为三态（见 <see cref="AssistantProbeResult"/>）：
/// "查询失败"必须以 <see cref="AssistantProbeOutcome.Unknown"/> 上报，绝不能与"确认无助手"混为一谈——
/// 否则一次瞬时 SDP 失败会被上层当成确定否定结论缓存，表现为"对端助手在线但本机识别不到，静默回退 OPP"。
/// </summary>
public static class AssistantDetector
{
    private static readonly Regex RemoteMacRegex = new(
        @"-([0-9A-Fa-f]{2}(?::[0-9A-Fa-f]{2}){5})(?:#|$)", RegexOptions.Compiled);

    /// <summary>已配对蓝牙设备（不依赖 OPP 广播；接收端只运行助手时也能解析）。</summary>
    public sealed record PairedAssistantDevice(string DeviceId, string Addr, string Name);

    /// <summary>已配对且运行接收助手服务的设备。</summary>
    public sealed record AssistantDeviceInfo(string DeviceId, string Addr, string Name);

    /// <summary>枚举已配对且声明助手服务（自定义 UUID）的设备，供发送端扫描合并展示。</summary>
    public static async Task<List<AssistantDeviceInfo>> FindAssistantDevicesAsync(CancellationToken ct = default)
    {
        var result = new List<AssistantDeviceInfo>();
        var devices = await DeviceInformation.FindAllAsync(
            BluetoothDevice.GetDeviceSelectorFromPairingState(true)).AsTask(ct);
        foreach (var d in devices)
        {
            ct.ThrowIfCancellationRequested();
            var mac = ParseMacFromId(d.Id);
            if (string.IsNullOrEmpty(mac)) continue;
            // 三态探测：查询失败的设备本轮跳过，但不产生"无助手"的确定结论（不会误导上层缓存）。
            var probe = await ProbeAssistantAsync(d.Id, ct);
            if (!probe.IsAvailable) continue;
            result.Add(new AssistantDeviceInfo(d.Id, mac, string.IsNullOrEmpty(d.Name) ? mac : d.Name));
        }
        return result;
    }

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

    /// <summary>
    /// 三态探测对端是否运行接收助手：
    /// <see cref="AssistantProbeOutcome.Available"/> = 查到服务；
    /// <see cref="AssistantProbeOutcome.Absent"/> = 两条查询路径均成功枚举但确认没有该服务；
    /// <see cref="AssistantProbeOutcome.Unknown"/> = 查询本身失败/设备 Id 缺失（<b>不得</b>被当作"无助手"缓存）。
    /// </summary>
    public static async Task<AssistantProbeResult> ProbeAssistantAsync(string deviceId, CancellationToken ct = default)
    {
        var (service, outcome, error) = await ResolveAssistantServiceAsync(deviceId, ct);
        service?.Dispose();
        return outcome switch
        {
            AssistantProbeOutcome.Available => AssistantProbeResult.Available,
            AssistantProbeOutcome.Absent => AssistantProbeResult.Absent,
            _ => AssistantProbeResult.Failed(error)
        };
    }

    /// <summary>
    /// 兼容入口：返回助手服务句柄或 null。
    /// 注意 null 同时代表"确认无助手"与"查询失败"，需要区分请改用 <see cref="ProbeAssistantAsync"/>。
    /// </summary>
    public static async Task<RfcommDeviceService?> GetAssistantServiceAsync(
        string deviceId, CancellationToken ct = default)
    {
        var (service, outcome, _) = await ResolveAssistantServiceAsync(deviceId, ct);
        if (outcome != AssistantProbeOutcome.Available)
        {
            service?.Dispose();
            return null;
        }
        return service;
    }

    /// <summary>
    /// 兼容入口：对端是否有助手。查询失败与确认无助手都返回 false（丢失原因，仅用于无需区分三态的老调用方）。
    /// </summary>
    public static async Task<bool> DeviceHasAssistantAsync(string deviceId, CancellationToken ct = default)
        => (await ProbeAssistantAsync(deviceId, ct)).IsAvailable;

    /// <summary>
    /// 解析助手服务。返回 (service, Available/Absent/Unknown, error)。
    /// 查询顺序：AEP 精确服务条目（主）→ 已配对设备 SDP（兜底，覆盖"已配对但不可发现"的对端）。
    /// </summary>
    private static async Task<(RfcommDeviceService? Service, AssistantProbeOutcome Outcome, string? Error)>
        ResolveAssistantServiceAsync(string deviceId, CancellationToken ct)
    {
        if (string.IsNullOrWhiteSpace(deviceId))
            return (null, AssistantProbeOutcome.Unknown, "设备未解析到 WinRT 设备 Id（可先扫描或配对）");

        var baseId = deviceId;
        var idx = baseId.IndexOf("#RFCOMM", StringComparison.OrdinalIgnoreCase);
        if (idx > 0) baseId = baseId[..idx];
        var targetMac = ParseMacFromId(baseId);
        var errors = new List<string>();

        // ---- 路径 1（主）：按服务 UUID 做 AEP 枚举，di.Id 是系统给出的精确服务条目 Id，
        //      无需猜测 RFCOMM 实例号，且 FromIdAsync(di.Id) 得到的服务句柄与父设备生命周期解耦。
        try
        {
            var selector = RfcommDeviceService.GetDeviceSelector(
                RfcommServiceId.FromUuid(AsstConst.ServiceUuid));
            var infos = await DeviceInformation.FindAllAsync(selector).AsTask(ct);
            foreach (var di in infos)
            {
                if (!MatchesBaseDevice(di.Id, baseId, targetMac)) continue;
                var service = await RfcommDeviceService.FromIdAsync(di.Id);
                if (service != null) return (service, AssistantProbeOutcome.Available, null);
                errors.Add($"AEP 命中但 FromIdAsync 失败：{di.Id}");
            }
            // AEP 枚举成功但未命中：不能就此判定 Absent——已配对但未开启可发现的对端不会出现在 AEP 里，
            // 必须继续走下面的 SDP 兜底（Windows 自带向导同样如此）。
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            errors.Add($"AEP 枚举失败：{ex.Message}");
        }

        // ---- 路径 2（兜底）：对已配对设备做 SDP 查询（能覆盖 AEP 枚举不到的对端）。
        BluetoothDevice? device = null;
        var keepDeviceAlive = false;
        try
        {
            device = await BluetoothDevice.FromIdAsync(baseId).AsTask(ct);
            if (device == null)
                return (null, AssistantProbeOutcome.Unknown,
                    $"BluetoothDevice.FromIdAsync 未解析到设备：{baseId}");

            var result = await device.GetRfcommServicesAsync(BluetoothCacheMode.Uncached).AsTask(ct);
            var match = result.Services.FirstOrDefault(s => s.ServiceId.Uuid == AsstConst.ServiceUuid);
            if (match == null)
            {
                // SDP 查询成功且确认没有该服务：这才是确定的否定结论。
                return (null, AssistantProbeOutcome.Absent, null);
            }

            // 优先用"该设备自身的服务接口条目"重建独立句柄（同样不猜实例号，句柄生命周期与父设备解耦）。
            var resolved = await ResolveFromDeviceInterfacesAsync(device, ct);
            if (resolved != null)
            {
                match.Dispose();
                return (resolved, AssistantProbeOutcome.Available, null);
            }

            // 罕见兜底：直接交出 SDP 枚举句柄，此时<b>不能</b>释放父 device（否则句柄可能失效，
            // 把"探测不到"变成"探测到但连接时炸"）。宁可让 GC 回收这一个 WinRT 句柄，也不返回不可用服务。
            keepDeviceAlive = true;
            return (match, AssistantProbeOutcome.Available, null);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch (Exception ex)
        {
            errors.Add(ex.Message);
            return (null, AssistantProbeOutcome.Unknown, string.Join("；", errors));
        }
        finally
        {
            if (!keepDeviceAlive) device?.Dispose();
        }
    }

    /// <summary>
    /// 枚举"某台已配对设备自身"的 RFCOMM 服务接口条目，按助手 UUID 取精确服务句柄。
    /// 失败返回 null（由调用方回退），不改变"已枚举到助手服务"这一事实。
    /// </summary>
    private static async Task<RfcommDeviceService?> ResolveFromDeviceInterfacesAsync(
        BluetoothDevice device, CancellationToken ct)
    {
        try
        {
            var infos = await DeviceInformation.FindAllAsync(
                RfcommDeviceService.GetDeviceSelectorForBluetoothDevice(device)).AsTask(ct);
            foreach (var info in infos)
            {
                var service = await RfcommDeviceService.FromIdAsync(info.Id);
                if (service == null) continue;
                if (service.ServiceId.Uuid == AsstConst.ServiceUuid) return service;
                service.Dispose();
            }
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            throw;
        }
        catch
        {
            // 回退：交给调用方使用 SDP 枚举句柄。
        }
        return null;
    }

    /// <summary>判定 AEP 服务条目的 Id 是否属于目标设备（优先设备 Id 前缀，其次 MAC 相等）。</summary>
    private static bool MatchesBaseDevice(string candidateId, string baseId, string targetMac)
    {
        if (string.IsNullOrEmpty(candidateId)) return false;
        if (!string.IsNullOrEmpty(baseId) &&
            candidateId.StartsWith(baseId, StringComparison.OrdinalIgnoreCase))
            return true;
        if (string.IsNullOrEmpty(targetMac)) return false;
        var mac = ParseMacFromId(candidateId);
        return !string.IsNullOrEmpty(mac) &&
               string.Equals(NormalizeAddr(mac), NormalizeAddr(targetMac), StringComparison.OrdinalIgnoreCase);
    }
}
