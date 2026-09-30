using System.Collections.ObjectModel;
using System.Windows.Threading;
using BluetoothTransfer.Core.Discovery;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using BluetoothTransfer.ViewModels;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// DevicePaneController 单元测试：全部经 fake 注入（IOppDiscoveryService / IStorageService /
/// Dispatcher / Devices 集合 / 状态与日志回调 / 助手探测与助手枚举委托），不触碰真实蓝牙静态调用。
/// 本批新增 ScanAsync 的助手探测-合并竞争回归：探测入口（probeAssistant）与助手枚举
/// （findAssistantDevices）均可注入 fake，因此无需蓝牙硬件即可复现"探测失败覆盖已知 true"。
/// 被测方法内部 await Task.Run(...)，延续经 DispatcherSynchronizationContext 回到 STA 线程后做集合操作，
/// 故测试体必须运行在「STA + Dispatcher 泵」上（见 RunOnDispatcherAsync）。
/// 注意：这些测试依赖 WPF/Dispatcher，只能在 Windows 上执行；纯策略断言见
/// <see cref="AssistantProbePolicyTests"/>（无 WPF 依赖，Linux 可跑）。
/// </summary>
public class DevicePaneControllerTests
{
    [Fact]
    public async Task ToggleFavorite_FirstFavorite_UpsertsAndMovesToTop()
    {
        await RunOnDispatcherAsync(async () =>
        {
            var fakeStorage = new FakeStorageService
            {
                // 收藏置顶排序依赖分数字段：此处无需真实收藏数据，仅提供空 devices。
            };
            var devices = new ObservableCollection<OppDeviceItem>
            {
                new OppDeviceItem { Name = "设备A", Addr = "aa:00:00:00:00:01", Favorite = false },
                new OppDeviceItem { Name = "设备B", Addr = "aa:00:00:00:00:02", Favorite = false },
            };

            var statuses = new List<string>();
            var controller = CreateController(fakeStorage, devices, statuses);

            var b = devices[1];
            await controller.ToggleFavoriteAsync(b);

            // 收藏：fake 收到 UpsertDevice，Favorite=true 且 Addr 为 B 的地址。
            var upsert = Assert.Single(fakeStorage.UpsertedDevices);
            Assert.True(upsert.Favorite);
            Assert.Equal(b.Addr, upsert.Addr);
            // 设备对象收藏位被置为 true。
            Assert.True(b.Favorite);
            // 收藏置顶排序：B 排在列表首位。
            Assert.Same(b, devices[0]);
            // 状态栏回写了「已收藏」。
            Assert.Contains(statuses, s => s.Contains("已收藏"));
            return Unit.Default;
        });
    }

    [Fact]
    public async Task ToggleFavorite_Unfavorite_CallsSetFavoriteFalse()
    {
        await RunOnDispatcherAsync(async () =>
        {
            var fakeStorage = new FakeStorageService();
            var devices = new ObservableCollection<OppDeviceItem>
            {
                new OppDeviceItem { Name = "设备B", Addr = "aa:00:00:00:00:02", Favorite = true },
            };

            var statuses = new List<string>();
            var controller = CreateController(fakeStorage, devices, statuses);

            var b = devices[0];
            await controller.ToggleFavoriteAsync(b);

            // 取消收藏：SetDeviceFavorite(B.Addr, false) 被调用。
            var setCall = Assert.Single(fakeStorage.SetFavoriteCalls);
            Assert.Equal(b.Addr, setCall.Addr);
            Assert.False(setCall.Favorite);
            // 设备对象收藏位被置为 false。
            Assert.False(b.Favorite);
            // 不触发 UpsertDevice（收藏路径）。
            Assert.Empty(fakeStorage.UpsertedDevices);
            return Unit.Default;
        });
    }

    /// <summary>
    /// 回归（Bug2 缺陷 3）：探测返回失败（Unknown）时，不得覆盖 MergeAssistantDevicesAsync 写入的 true。
    /// 修复前 ProbeAssistantAsync 无条件 item.IsAssistant = has（has=false）会与合并且序竞争，徽标消失。
    /// </summary>
    [Fact]
    public async Task Scan_ProbeFailure_DoesNotEraseAssistantBadgeFromMerge()
    {
        await RunOnDispatcherAsync(async () =>
        {
            const string addr = "aa:00:00:00:00:01";
            var devices = new ObservableCollection<OppDeviceItem>();
            var tracker = new AssistantPresenceTracker();
            var probed = new List<string>();
            var logs = new List<(string Level, string Message)>();

            var controller = new DevicePaneController(
                new StubDiscovery(new List<OppDeviceInfo>
                {
                    new() { Id = "dev-1", Addr = addr, Name = "对端", IsPaired = true }
                }),
                new FakeStorageService(),
                Dispatcher.CurrentDispatcher,
                devices,
                tracker,
                () => false,
                _ => { },
                _ => { },
                _ => { },
                (lvl, msg) => logs.Add((lvl, msg)),
                probeAssistant: id => { probed.Add(id); return Task.FromResult(AssistantProbeResult.Failed("设备忙")); },
                findAssistantDevices: _ => Task.FromResult(new List<AssistantDetector.AssistantDeviceInfo>
                {
                    new("dev-1", addr, "对端")
                }));

            await controller.ScanAsync();

            // 探测确实执行过（失败），但合并的 true 必须保留。
            Assert.Equal("dev-1", Assert.Single(probed));
            var item = Assert.Single(devices);
            Assert.True(item.IsAssistant);
            Assert.True(tracker.Current(addr).IsAvailable);
            return Unit.Default;
        });
    }

    /// <summary>
    /// 回归（Bug2 缺陷 1/2）：探测到助手应点亮徽标；随后的瞬时失败（Unknown）不得把已确认的 true 降级。
    /// </summary>
    [Fact]
    public async Task Scan_ProbeAvailable_SetsBadge_AndLaterFailureKeepsIt()
    {
        await RunOnDispatcherAsync(async () =>
        {
            const string addr = "aa:00:00:00:00:02";
            var devices = new ObservableCollection<OppDeviceItem>();
            var tracker = new AssistantPresenceTracker();
            var controller = new DevicePaneController(
                new StubDiscovery(new List<OppDeviceInfo>
                {
                    new() { Id = "dev-2", Addr = addr, Name = "助手端", IsPaired = true }
                }),
                new FakeStorageService(),
                Dispatcher.CurrentDispatcher,
                devices,
                tracker,
                () => false,
                _ => { },
                _ => { },
                _ => { },
                (_, _) => { },
                probeAssistant: _ => Task.FromResult(AssistantProbeResult.Available),
                findAssistantDevices: _ => Task.FromResult(new List<AssistantDetector.AssistantDeviceInfo>()));

            await controller.ScanAsync();
            var item = Assert.Single(devices);
            Assert.True(item.IsAssistant);

            // 修复前这里会被无条件写成 false。
            controller.ApplyProbeResult(item, AssistantProbeResult.Failed("设备忙/休眠"));
            Assert.True(item.IsAssistant);
            Assert.True(tracker.Current(addr).IsAvailable);
            return Unit.Default;
        });
    }

    /// <summary>
    /// 回归（Bug2 缺陷 1）：确定的否定结果（Absent）应把徽标置为 false，但不得作为长期缓存结论
    /// （tracker.Current 仍返回 Unknown，下一次探测/发送会重探）。
    /// </summary>
    [Fact]
    public async Task Scan_ProbeAbsent_ClearsBadgeButStaysReprobeable()
    {
        await RunOnDispatcherAsync(() =>
        {
            const string addr = "aa:00:00:00:00:03";
            var devices = new ObservableCollection<OppDeviceItem> { new() { Addr = addr, DeviceId = "dev-3", IsAssistant = true } };
            var tracker = new AssistantPresenceTracker();
            var controller = new DevicePaneController(
                new StubDiscovery(new List<OppDeviceInfo>()),
                new FakeStorageService(),
                Dispatcher.CurrentDispatcher,
                devices,
                tracker,
                () => false,
                _ => { },
                _ => { },
                _ => { },
                (_, _) => { },
                probeAssistant: _ => Task.FromResult(AssistantProbeResult.Absent),
                findAssistantDevices: _ => Task.FromResult(new List<AssistantDetector.AssistantDeviceInfo>()));

            controller.ApplyProbeResult(devices[0], AssistantProbeResult.Absent);

            Assert.False(devices[0].IsAssistant);
            // 负结果不作为缓存结论：Current 仍为 Unknown（可重探），避免"一次否定锁死整个会话"。
            Assert.True(tracker.Current(addr).IsUnknown);
            return Task.FromResult(Unit.Default);
        });
    }

    private static DevicePaneController CreateController(
        IStorageService storage,
        ObservableCollection<OppDeviceItem> devices,
        List<string> statuses)
    {
        var statusCalls = statuses;
        return new DevicePaneController(
            new FakeDiscovery(),
            storage,
            Dispatcher.CurrentDispatcher,
            devices,
            new AssistantPresenceTracker(),
            () => false,
            _ => { },
            s => statusCalls.Add(s),
            _ => { },
            (lvl, msg) => { });
    }

    /// <summary>经典 WPF 单测模式：STA 线程 + Dispatcher.InvokeAsync + Dispatcher.Run 泵。</summary>
    private static Task<T> RunOnDispatcherAsync<T>(Func<Task<T>> action)
    {
        T? result = default;
        Exception? error = null;
        var thread = new Thread(() =>
        {
            var dispatcher = Dispatcher.CurrentDispatcher;
            var priorSyncContext = SynchronizationContext.Current;
            SynchronizationContext.SetSynchronizationContext(new DispatcherSynchronizationContext(dispatcher));
            try
            {
                _ = dispatcher.InvokeAsync(async () =>
                {
                    try { result = await action(); }
                    catch (Exception ex) { error = ex; }
                    finally { dispatcher.InvokeShutdown(); }
                });
                Dispatcher.Run();
            }
            finally
            {
                SynchronizationContext.SetSynchronizationContext(priorSyncContext);
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        thread.Join();
        if (error != null) return Task.FromException<T>(error);
        return Task.FromResult(result!);
    }

    private record struct Unit
    {
        public static Unit Default => default;
    }
}

/// <summary>
/// IOppDiscoveryService 的 fake：本批不测扫描/配对，DiscoverAsync/PairAsync 直接抛 NotSupportedException。
/// </summary>
internal sealed class FakeDiscovery : IOppDiscoveryService
{
    public Task<List<OppDeviceInfo>> DiscoverAsync(bool pairedOnly = false, int seconds = 0, CancellationToken ct = default)
        => throw new NotSupportedException("本批测试不覆盖设备发现。");

    public Task<bool> PairAsync(string addr, string? pin = null, CancellationToken ct = default)
        => throw new NotSupportedException("本批测试不覆盖设备配对。");
}

/// <summary>返回预设设备列表的 IOppDiscoveryService fake：供 ScanAsync 助手探测-合并回归使用。</summary>
internal sealed class StubDiscovery : IOppDiscoveryService
{
    private readonly List<OppDeviceInfo> _devices;

    public StubDiscovery(List<OppDeviceInfo> devices) => _devices = devices;

    public Task<List<OppDeviceInfo>> DiscoverAsync(bool pairedOnly = false, int seconds = 0, CancellationToken ct = default)
        => Task.FromResult(new List<OppDeviceInfo>(_devices));

    public Task<bool> PairAsync(string addr, string? pin = null, CancellationToken ct = default)
        => Task.FromResult(false);
}
