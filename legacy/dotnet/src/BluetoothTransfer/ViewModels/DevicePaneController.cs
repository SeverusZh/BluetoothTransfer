using System.Collections.ObjectModel;
using System.Windows.Threading;
using BluetoothTransfer.Core.Discovery;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.ViewModels;

/// <summary>
/// 设备窗格控制器：承载设备扫描/配对/收藏/助手探测与合并的纯内部逻辑
/// （ScanAsync/PairAsync/ToggleFavoriteAsync/ProbeAssistantAsync/MergeAssistantDevicesAsync/
/// LoadFavoriteDevicesAsync/ResortDevices/RefreshPairedStatusAsync）。
/// 它为 <see cref="OppViewModel"/> 的一窗格职责做组合式拆分，避免"上帝 ViewModel"继续膨胀。
///
/// 约束与设计：
/// 1. Devices 集合对象本身仍由 ViewModel 暴露（XAML 绑定），本控制器只通过构造注入的集合引用做内部操作；
/// 2. _assistantTracker 是与 ViewModel 发送队列（ResolveAssistantOnceAsync）共用的同一实例，
///    由 ViewModel 在构造函数传入引用；探测结论（TTL 重探、"失败不得降级已知 true"）由纯类型
///    AssistantPresenceTracker 统一裁决，两域只做薄适配；
/// 3. 不反向依赖 ViewModel：凡需回写 VM 状态（IsScanning/StatusText/LastError）或发布日志处，
///    一律经构造注入的回调委托，避免本类反向引用 VM。
/// </summary>
internal sealed class DevicePaneController
{
    private readonly IOppDiscoveryService _discovery;
    private readonly IStorageService _storage;
    private readonly Dispatcher _dispatcher;
    private readonly ObservableCollection<OppDeviceItem> _devices;
    private readonly AssistantPresenceTracker _assistantTracker;
    private readonly Func<string, Task<AssistantProbeResult>> _probeAssistant;
    private readonly Func<CancellationToken, Task<List<AssistantDetector.AssistantDeviceInfo>>> _findAssistantDevices;
    private readonly Func<bool> _isScanning;
    private readonly Action<bool> _setScanning;
    private readonly Action<string> _setStatus;
    private readonly Action<string> _setLastError;
    private readonly Action<string, string> _publishLog;

    public DevicePaneController(
        IOppDiscoveryService discovery,
        IStorageService storage,
        Dispatcher dispatcher,
        ObservableCollection<OppDeviceItem> devices,
        AssistantPresenceTracker assistantTracker,
        Func<bool> isScanning,
        Action<bool> setScanning,
        Action<string> setStatus,
        Action<string> setLastError,
        Action<string, string> publishLog,
        Func<string, Task<AssistantProbeResult>>? probeAssistant = null,
        Func<CancellationToken, Task<List<AssistantDetector.AssistantDeviceInfo>>>? findAssistantDevices = null)
    {
        _discovery = discovery;
        _storage = storage;
        _dispatcher = dispatcher;
        _devices = devices;
        _assistantTracker = assistantTracker;
        // 探测入口可注入（单测用 fake，避免真实 SDP/WinRT 调用）。
        _probeAssistant = probeAssistant ?? (id => AssistantDetector.ProbeAssistantAsync(id));
        // 助手设备枚举同样可注入（单测用 fake，避免真实 WinRT 调用）。
        _findAssistantDevices = findAssistantDevices ?? (ct => AssistantDetector.FindAssistantDevicesAsync(ct));
        _isScanning = isScanning;
        _setScanning = setScanning;
        _setStatus = setStatus;
        _setLastError = setLastError;
        _publishLog = publishLog;
    }

    public async Task ScanAsync()
    {
        if (_isScanning()) return;
        _setScanning(true);
        try
        {
            _setStatus("正在扫描支持 OPP 的设备（5 秒）...");
            var devices = await Task.Run(() => _discovery.DiscoverAsync(seconds: 5));
            var saved = _storage.GetDevices();
            List<OppDeviceItem> pendingProbes = new();
            _dispatcher.Invoke(() =>
            {
                _devices.Clear();
                _assistantTracker.Clear();
                var items = devices.Select(d =>
                {
                    var info = saved.FirstOrDefault(s => OppDiscoveryService.AddrEquals(s.Addr, d.Addr));
                    return new OppDeviceItem
                    {
                        Name = d.Name,
                        Addr = d.Addr,
                        DeviceId = d.Id,
                        IsPaired = d.IsPaired,
                        Favorite = info?.Favorite ?? false,
                        Alias = info?.Alias ?? "",
                        LastConnected = info?.LastConnected ?? ""
                    };
                })
                .OrderByDescending(x => x.Favorite)
                .ThenByDescending(x => x.IsPaired)
                .ThenByDescending(x => x.LastConnected)
                .ToList();
                foreach (var item in items) _devices.Add(item);
                pendingProbes = _devices.ToList();
            });
            // 先 await 完本轮所有探测再合并助手设备：原实现 fire-and-forget 会让并发探测返回的
            // 过期 false 覆盖 MergeAssistantDevicesAsync 写入的 true，表现为助手徽标"识别不到"。
            await Task.WhenAll(pendingProbes.Select(ProbeAssistantAsync));
            await MergeAssistantDevicesAsync(saved);
            await LoadFavoriteDevicesAsync();
            _setStatus(_devices.Count > 0
                ? $"发现 {_devices.Count} 个 OPP 设备"
                : "未发现 OPP 设备（确认对端已开启蓝牙并处于可发现状态，或先在系统设置中配对）");
        }
        catch (Exception ex)
        {
            _setLastError(ex.Message);
            _setStatus($"扫描失败：{ex.Message}");
        }
        finally
        {
            _setScanning(false);
        }
    }

    public async Task PairAsync(OppDeviceItem? device)
    {
        if (device == null) return;
        _setStatus($"正在配对 {device.DisplayName}...");
        var ok = await _discovery.PairAsync(device.Addr);
        _setStatus(ok
            ? $"配对成功：{device.DisplayName}"
            : $"配对失败：{device.DisplayName}（可尝试在系统设置中手动配对）");
        await RefreshPairedStatusAsync();
    }

    private async Task RefreshPairedStatusAsync()
    {
        try
        {
            var devices = await Task.Run(() => _discovery.DiscoverAsync(seconds: 0));
            _dispatcher.Invoke(() =>
            {
                foreach (var item in _devices)
                {
                    var match = devices.FirstOrDefault(d => OppDiscoveryService.AddrEquals(d.Addr, item.Addr));
                    if (match != null) item.IsPaired = match.IsPaired;
                }
            });
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogError, $"刷新配对状态失败：{ex.Message}");
        }
    }

    public async Task ToggleFavoriteAsync(OppDeviceItem? device)
    {
        if (device == null) return;
        var next = !device.Favorite;
        if (next)
        {
            // 收藏：插入或更新记录（对从未发送过的设备也能持久化）
            await Task.Run(() => _storage.UpsertDevice(new DeviceInfo
            {
                Addr = device.Addr,
                Name = device.Name,
                Alias = device.Alias,
                Favorite = true,
                LastSeen = DateTime.Now.ToString("o")
            }));
        }
        else
        {
            await Task.Run(() => _storage.SetDeviceFavorite(device.Addr, false));
        }
        device.Favorite = next;
        ResortDevices();
        _setStatus(next ? $"已收藏：{device.DisplayName}" : $"已取消收藏：{device.DisplayName}");
    }

    /// <summary>收藏置顶 → 已配对 → 最近连接 的展示排序。</summary>
    private void ResortDevices()
    {
        var sorted = _devices
            .OrderByDescending(d => d.Favorite)
            .ThenByDescending(d => d.IsPaired)
            .ThenByDescending(d => d.LastConnected)
            .ToList();
        for (var i = 0; i < sorted.Count; i++)
            _devices.Move(_devices.IndexOf(sorted[i]), i);
    }

    /// <summary>
    /// 合并"已配对且运行接收助手"的设备：接收端只跑 btrecv（不广播 OPP）时也能在列表中出现。
    /// </summary>
    private async Task MergeAssistantDevicesAsync(List<DeviceInfo> saved)
    {
        List<AssistantDetector.AssistantDeviceInfo> assistantDevices;
        try
        {
            assistantDevices = await Task.Run(() => _findAssistantDevices(CancellationToken.None));
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogWarn, $"枚举助手设备失败：{ex.Message}");
            return;
        }

        _dispatcher.Invoke(() =>
        {
            foreach (var ad in assistantDevices)
            {
                var existing = _devices.FirstOrDefault(d => OppDiscoveryService.AddrEquals(d.Addr, ad.Addr));
                if (existing != null)
                {
                    existing.IsAssistant = true;
                }
                else
                {
                    var info = saved.FirstOrDefault(s => OppDiscoveryService.AddrEquals(s.Addr, ad.Addr));
                    _devices.Add(new OppDeviceItem
                    {
                        Name = ad.Name,
                        Addr = ad.Addr,
                        DeviceId = ad.DeviceId,
                        IsPaired = true,
                        IsAssistant = true,
                        Favorite = info?.Favorite ?? false,
                        Alias = info?.Alias ?? "",
                        LastConnected = info?.LastConnected ?? ""
                    });
                }
                // 合并结果来自"已配对且确认声明助手服务"的枚举，是正结果（同时刷新 TTL）。
                _assistantTracker.Record(ad.Addr, AssistantProbeResult.Available);
            }
            ResortDevices();
        });
    }

    /// <summary>
    /// 收藏夹：把本地保存的收藏设备合并进列表，无需每次扫描即可选中发送。
    /// 地址经已配对列表解析 DeviceId（用于助手探测），未配对时发送会给出明确错误。
    /// </summary>
    public async Task LoadFavoriteDevicesAsync()
    {
        try
        {
            var favorites = await Task.Run(() => _storage.GetDevices().Where(d => d.Favorite).ToList());
            var items = new List<OppDeviceItem>();
            foreach (var f in favorites)
            {
                var paired = await AssistantDetector.FindPairedDeviceAsync(f.Addr);
                items.Add(new OppDeviceItem
                {
                    Name = f.Name,
                    Addr = f.Addr,
                    DeviceId = paired?.DeviceId ?? "",
                    IsPaired = paired != null,
                    Favorite = true,
                    Alias = f.Alias ?? "",
                    LastConnected = f.LastConnected ?? ""
                });
            }
            _dispatcher.Invoke(() =>
            {
                foreach (var item in items)
                {
                    if (_devices.Any(d => OppDiscoveryService.AddrEquals(d.Addr, item.Addr))) continue;
                    _devices.Add(item);
                    _ = ProbeAssistantAsync(item);
                }
                ResortDevices();
            });
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogWarn, $"加载收藏设备失败：{ex.Message}");
        }
    }

    private async Task ProbeAssistantAsync(OppDeviceItem item)
    {
        AssistantProbeResult result;
        try
        {
            result = await Task.Run(() => _probeAssistant(item.DeviceId));
        }
        catch (Exception ex)
        {
            // 探测异常同样按"查询失败"处理：记 Unknown，不降级已知 true。
            result = AssistantProbeResult.Failed(ex.Message);
            _publishLog(TransferConst.LogWarn, $"探测助手失败：{item.DisplayName}（{ex.Message}）");
        }
        ApplyProbeResult(item, result);
    }

    /// <summary>
    /// 把一次探测结果落到跟踪器与 UI 徽标（薄适配）：
    /// Available → 置 true；Absent（查询成功且确认没有）→ 置 false；
    /// Unknown（查询失败）→ 保持现状，绝不把已知 true 降级。
    /// </summary>
    internal void ApplyProbeResult(OppDeviceItem item, AssistantProbeResult result)
    {
        var effective = _assistantTracker.Record(item.Addr, result);
        _dispatcher.Invoke(() =>
        {
            if (effective.IsAvailable) item.IsAssistant = true;
            else if (effective.IsAbsent) item.IsAssistant = false;
        });
    }
}
