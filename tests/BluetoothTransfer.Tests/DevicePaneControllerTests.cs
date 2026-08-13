using System.Collections.ObjectModel;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using BluetoothTransfer.ViewModels;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// DevicePaneController 单元测试：全部经 fake 注入（IOppDiscoveryService / IStorageService /
/// Dispatcher / Devices 集合 / 状态与日志回调），不触碰真实蓝牙静态调用，
/// 也本批不测 Scan/Pair/Probe（涉及真实蓝牙静态调用）。
/// 被测方法 ToggleFavoriteAsync 内部 await Task.Run(...)，延续经 DispatcherSynchronizationContext
/// 回到 STA 线程后做集合操作，故测试体必须运行在「STA + Dispatcher 泵」上（见 RunOnDispatcherAsync）。
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
            new Dictionary<string, bool>(StringComparer.OrdinalIgnoreCase),
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
