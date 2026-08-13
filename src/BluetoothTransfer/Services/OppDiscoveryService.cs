using System.Diagnostics;
using System.Text.RegularExpressions;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Devices.Enumeration;
using Windows.Foundation;

namespace BluetoothTransfer.Services;

/// <summary>支持 OPP（蓝牙文件接收）的设备信息。</summary>
public sealed class OppDeviceInfo
{
    public string Id { get; init; } = "";
    public string Addr { get; init; } = "";
    public string Name { get; init; } = "";
    public bool IsPaired { get; init; }

    public string AddrDisplay => OppDiscoveryService.NormalizeAddr(Addr);
}

/// <summary>
/// OPP 设备发现与配对：枚举支持 Object Push（UUID 0x1105）的经典蓝牙设备，
/// 并通过系统配对 API 发起配对（ConfirmOnly / ProvidePin）。
///
/// 兼容性说明：Windows 对 RFCOMM 服务条目（AEP）返回的 DeviceInformation 中
/// <c>System.Devices.Aep.DeviceAddress</c> 属性可能缺失、<c>Pairing</c> 可能为 null，
/// 且 <c>Pairing.IsPaired</c> 并不可靠；因此统一从设备 Id 解析远程 MAC，
/// 并以"已配对设备列表"（GetDeviceSelectorFromPairingState(true)）作为配对状态依据。
/// </summary>
public sealed class OppDiscoveryService
{
    private static readonly Regex RemoteMacRegex =
        new(@"-([0-9A-Fa-f]{2}(?::[0-9A-Fa-f]{2}){5})(?:#|$)", RegexOptions.Compiled);

    private readonly EventBus _events;

    public OppDiscoveryService(EventBus events)
    {
        _events = events ?? throw new ArgumentNullException(nameof(events));
    }

    /// <summary>
    /// 扫描支持 OPP 的设备。发现时长由 <paramref name="seconds"/> 控制（0 表示单次枚举）。
    /// </summary>
    public async Task<List<OppDeviceInfo>> DiscoverAsync(bool pairedOnly = false, int seconds = 0, CancellationToken ct = default)
    {
        var selector = RfcommDeviceService.GetDeviceSelector(RfcommServiceId.ObexObjectPush);
        var found = new Dictionary<string, OppDeviceInfo>(StringComparer.OrdinalIgnoreCase);
        var pairedMap = await GetPairedDeviceMapAsync();
        var deadline = DateTime.UtcNow.AddSeconds(Math.Max(0, seconds));

        try
        {
            do
            {
                ct.ThrowIfCancellationRequested();
                var devices = await DeviceInformation.FindAllAsync(selector).AsTask(ct);
                foreach (var di in devices)
                {
                    var addr = GetDeviceAddress(di);
                    if (string.IsNullOrEmpty(addr)) continue;
                    var key = NormalizeAddr(addr);
                    var isPaired = pairedMap.TryGetValue(key, out var pairedInfo);
                    var name = isPaired
                        ? pairedInfo.Name
                        : await ResolveDeviceNameAsync(di);

                    found[key] = new OppDeviceInfo
                    {
                        Id = di.Id,
                        Addr = addr,
                        Name = string.IsNullOrEmpty(name) ? key : name,
                        IsPaired = isPaired
                    };
                }
                // 已配对设备可能未处于"可发现"状态，AEP 服务枚举不会返回它们；
                // 直接对每个已配对设备做一次 SDP 查询，Windows 自带向导也是这么工作的。
                await AddPairedOppServicesAsync(found, ct);
                if (seconds <= 0 || DateTime.UtcNow >= deadline) break;
                await Task.Delay(500, ct);
            } while (true);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"OPP 设备扫描失败：{ex.Message}"));
        }

        var result = found.Values.ToList();
        return pairedOnly ? result.Where(d => d.IsPaired).ToList() : result;
    }

    /// <summary>按地址查找 OPP 设备（含未配对，由调用方决定如何处理）。</summary>
    public async Task<OppDeviceInfo?> FindByAddressAsync(string addr, CancellationToken ct = default)
    {
        var devices = await DiscoverAsync(pairedOnly: false, seconds: 0, ct: ct);
        return devices.FirstOrDefault(d => AddrEquals(d.Addr, addr));
    }

    /// <summary>发起配对：未提供 PIN 时走 ConfirmOnly（系统确认），提供 PIN 时走 ProvidePin。</summary>
    public async Task<bool> PairAsync(string addr, string? pin = null, CancellationToken ct = default)
    {
        var devices = await DiscoverAsync(seconds: 0, ct: ct);
        var device = devices.FirstOrDefault(d => AddrEquals(d.Addr, addr));
        if (device == null)
        {
            _events.Publish(new LogEvent("ERROR", $"未找到可配对的 OPP 设备：{addr}"));
            return false;
        }
        return await PairByDeviceAsync(device.Id, pin, ct);
    }

    public async Task<bool> PairByDeviceAsync(string deviceId, string? pin, CancellationToken ct = default)
    {
        // 已配对设备列表比 AEP 的 Pairing.IsPaired 可靠
        var pairedMap = await GetPairedDeviceMapAsync();
        var mac = ParseMacFromId(deviceId);
        if (!string.IsNullOrEmpty(mac) && pairedMap.ContainsKey(NormalizeAddr(mac)))
        {
            _events.Publish(new LogEvent("INFO", "设备已配对"));
            return true;
        }

        // 服务条目（... #RFCOMM:...）上无法直接配对，需要回退到蓝牙设备条目
        var bluetoothId = GetBluetoothDeviceId(deviceId);
        DeviceInformation? info = null;
        try
        {
            info = await DeviceInformation.CreateFromIdAsync(bluetoothId).AsTask(ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"获取设备信息失败：{ex.Message}"));
            return false;
        }
        if (info?.Pairing == null)
        {
            _events.Publish(new LogEvent("ERROR", "无法获取设备配对接口，请在 Windows 蓝牙设置中手动配对"));
            return false;
        }

        var custom = info.Pairing.Custom;
        TypedEventHandler<DeviceInformationCustomPairing, DevicePairingRequestedEventArgs> handler =
            (_, args) =>
            {
                // pin 非空时走 ProvidePin 流程并提供 PIN（DevicePairingRequestedEventArgs.Accept(string) 重载），
                // pin 为空时维持 ConfirmOnly 流程（args.Accept() 无参重载）。
                if (!string.IsNullOrEmpty(pin))
                    args.Accept(pin);
                else
                    args.Accept();
            };
        custom.PairingRequested += handler;
        try
        {
            var kinds = string.IsNullOrEmpty(pin)
                ? DevicePairingKinds.ConfirmOnly
                : DevicePairingKinds.ProvidePin;
            var result = await custom.PairAsync(kinds, DevicePairingProtectionLevel.Encryption).AsTask(ct);
            var ok = result.Status == DevicePairingResultStatus.Paired;
            _events.Publish(new LogEvent(ok ? "INFO" : "WARN", $"配对结果：{result.Status}"));
            return ok;
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"配对失败：{ex.Message}"));
            return false;
        }
        finally
        {
            custom.PairingRequested -= handler;
        }
    }

    public static void OpenBluetoothSettings(EventBus events)
    {
        try
        {
            Process.Start(new ProcessStartInfo("ms-settings:bluetooth") { UseShellExecute = true });
        }
        catch (Exception ex)
        {
            events.Publish(new LogEvent("ERROR", $"打开蓝牙设置失败：{ex.Message}"));
        }
    }

    public static string NormalizeAddr(string addr)
    {
        if (string.IsNullOrEmpty(addr)) return "";
        var clean = addr.Replace(":", "").Replace("-", "").Trim();
        if (clean.Length != 12) return addr;
        return string.Join(":", Enumerable.Range(0, 6).Select(i => clean.Substring(i * 2, 2))).ToLowerInvariant();
    }

    public static bool AddrEquals(string a, string b)
        => string.Equals(NormalizeAddr(a), NormalizeAddr(b), StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// 从 WinRT 蓝牙设备 Id 中解析远程 MAC。
    /// Id 形如 <c>Bluetooth#Bluetooth&lt;本机MAC&gt;-&lt;对端MAC&gt;[#RFCOMM:...]</c>，
    /// 对端 MAC 是紧跟第一个（也是唯一一个）"-" 的 17 字符地址。
    /// </summary>
    internal static string ParseMacFromId(string id)
    {
        if (string.IsNullOrEmpty(id)) return "";
        var match = RemoteMacRegex.Match(id);
        return match.Success ? match.Groups[1].Value : "";
    }

    /// <summary>去掉 RFCOMM 服务后缀，得到蓝牙设备条目 Id（用于配对）。</summary>
    internal static string GetBluetoothDeviceId(string serviceId)
    {
        if (string.IsNullOrEmpty(serviceId)) return "";
        var idx = serviceId.IndexOf("#RFCOMM", StringComparison.OrdinalIgnoreCase);
        return idx > 0 ? serviceId[..idx] : serviceId;
    }

    private static string GetDeviceAddress(DeviceInformation di)
    {
        if (di.Properties.TryGetValue("System.Devices.Aep.DeviceAddress", out var value) && value is string s && !string.IsNullOrEmpty(s))
            return s;
        return ParseMacFromId(di.Id);
    }

    private static async Task<string> ResolveDeviceNameAsync(DeviceInformation di)
    {
        // 服务条目的 Name 通常是服务名（如 "OBEX Object Push"），尝试获取真实设备名
        try
        {
            var service = await RfcommDeviceService.FromIdAsync(di.Id);
            if (service?.Device != null && !string.IsNullOrEmpty(service.Device.Name))
                return service.Device.Name;
        }
        catch
        {
            // 名称解析失败时回退
        }
        return di.Name;
    }

    /// <summary>枚举系统"已配对"经典蓝牙设备，按规范化 MAC 建立 名称/配对状态 映射。</summary>
    private static async Task<Dictionary<string, (string Name, bool Paired)>> GetPairedDeviceMapAsync()
    {
        var map = new Dictionary<string, (string Name, bool Paired)>(StringComparer.OrdinalIgnoreCase);
        try
        {
            var paired = await DeviceInformation.FindAllAsync(BluetoothDevice.GetDeviceSelectorFromPairingState(true));
            foreach (var d in paired)
            {
                var mac = ParseMacFromId(d.Id);
                if (string.IsNullOrEmpty(mac)) continue;
                map[NormalizeAddr(mac)] = (string.IsNullOrEmpty(d.Name) ? mac : d.Name, true);
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"枚举已配对设备失败：{ex.Message}");
        }
        return map;
    }

    /// <summary>
    /// 对已配对经典蓝牙设备逐个做 SDP 查询，把声明了 OBEX Object Push（0x1105）的设备
    /// 补进结果。AEP 服务枚举只返回"可发现"设备，已配对但未开启可发现的设备（例如
    /// Windows 对端）即使正在运行接收向导也不会出现在枚举里，因此需要直接查询。
    /// </summary>
    private async Task AddPairedOppServicesAsync(Dictionary<string, OppDeviceInfo> found, CancellationToken ct)
    {
        try
        {
            var paired = await DeviceInformation.FindAllAsync(
                BluetoothDevice.GetDeviceSelectorFromPairingState(true)).AsTask(ct);
            foreach (var d in paired)
            {
                ct.ThrowIfCancellationRequested();
                var mac = ParseMacFromId(d.Id);
                if (string.IsNullOrEmpty(mac)) continue;
                var key = NormalizeAddr(mac);
                if (found.ContainsKey(key)) continue;

                BluetoothDevice? dev = null;
                try
                {
                    dev = await BluetoothDevice.FromIdAsync(d.Id).AsTask(ct);
                    if (dev == null) continue;
                    var result = await dev.GetRfcommServicesAsync(BluetoothCacheMode.Uncached).AsTask(ct);
                    foreach (var svc in result.Services)
                    {
                        if (svc.ServiceId.Uuid != RfcommServiceId.ObexObjectPush.Uuid) continue;
                        // 构造服务条目 Id（与 AEP 枚举返回的格式一致），供后续 FromIdAsync 使用
                        var serviceId = d.Id + "#RFCOMM:00000000:{" + RfcommServiceId.ObexObjectPush.Uuid + "}";
                        var name = string.IsNullOrEmpty(d.Name) ? key : d.Name;
                        found[key] = new OppDeviceInfo
                        {
                            Id = serviceId,
                            Addr = mac,
                            Name = name,
                            IsPaired = true
                        };
                        _events.Publish(new LogEvent("DEBUG", $"已配对设备 SDP 预检：{name} 声明 OPP（0x1105）"));
                        break;
                    }
                }
                catch (Exception ex)
                {
                    // 设备不在范围内 / 休眠 / 关闭等均属正常，忽略并继续
                    _events.Publish(new LogEvent("DEBUG", $"已配对设备 SDP 预检跳过 {mac}：{ex.Message}"));
                }
                finally
                {
                    dev?.Dispose();
                }
            }
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("WARN", $"已配对设备 OPP 预检失败：{ex.Message}"));
        }
    }
}
