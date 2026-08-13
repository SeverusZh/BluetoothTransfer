using System.Collections.ObjectModel;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using BluetoothTransfer.ViewModels;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// RecordsPaneController 单元测试：全部经 fake 注入（IStorageService / Dispatcher / ObservableCollection / 回调），
/// 不触碰表达式中的 SaveFileDialog / MessageBox / 真实存储。
/// 控制器内部 LoadCoreAsync 经 Task.Run + _dispatcher.Invoke 完成加载与回写，
/// 故测试体必须运行在「STA 线程 + Dispatcher 泵」上，否则 await 延续会落线程池、
/// 而 _dispatcher.Invoke 会阻塞等待一个无人泵消息的 Dispatcher，造成死锁。
/// 因此每个测试经 <see cref="RunOnDispatcherAsync{T}"/> 执行。
/// </summary>
public class RecordsPaneControllerTests
{
    [Fact]
    public async Task LoadRecordsAsync_PopulatesRecordsInOrder()
    {
        await RunOnDispatcherAsync(async () =>
        {
            var data1 = new TransferRecord { Id = 1, PeerName = "设备A", Direction = TransferConst.DirSend };
            var data2 = new TransferRecord { Id = 2, PeerName = "设备B", Direction = TransferConst.DirRecv };
            var fake = new FakeStorageService { Records = { data1, data2 } };

            var records = new ObservableCollection<TransferRecord>();
            var logs = new List<(string Level, string Message)>();
            var statuses = new List<string>();
            var controller = new RecordsPaneController(fake, Dispatcher.CurrentDispatcher, records,
                s => statuses.Add(s), (lvl, msg) => logs.Add((lvl, msg)));

            await controller.LoadRecordsAsync();

            // 断言顺序与内容：fake 返回两条，按序填入 Records。
            Assert.Collection(records,
                r => Assert.Same(data1, r),
                r => Assert.Same(data2, r));
            Assert.Equal("设备A", records[0].PeerName);
            Assert.Equal("设备B", records[1].PeerName);
            // 加载仅调用一次 GetRecords。
            Assert.Equal(1, fake.GetRecordsCalls);
            return Unit.Default;
        });
    }

    [Fact]
    public async Task LoadRecordsAsync_ConcurrentCalls_LatestWins()
    {
        await RunOnDispatcherAsync(async () =>
        {
            var gate = new TaskCompletionSource<object?>(TaskCreationOptions.RunContinuationsAsynchronously);
            var data1 = new TransferRecord { Id = 1, PeerName = "旧数据" };
            var data2 = new TransferRecord { Id = 2, PeerName = "新数据" };

            var fake = new FakeStorageService();
            // 第一次 GetRecords 等待门控后返回 data1；之后再调用（reload 触发）返回 data2。
            var seq = new Queue<Func<List<TransferRecord>>>();
            seq.Enqueue(() =>
            {
                gate.Task.GetAwaiter().GetResult();
                return new List<TransferRecord> { data1 };
            });
            seq.Enqueue(() => new List<TransferRecord> { data2 });
            fake.GetRecordsSequence = seq;

            var records = new ObservableCollection<TransferRecord>();
            var logs = new List<(string Level, string Message)>();
            var controller = new RecordsPaneController(fake, Dispatcher.CurrentDispatcher, records,
                _ => { }, (lvl, msg) => logs.Add((lvl, msg)));

            // 先启动第一次加载（内部在门控上等待），再在它返回前发起第二次加载，应复用同一底层任务。
            var first = controller.LoadRecordsAsync();
            var second = controller.LoadRecordsAsync();
            Assert.Same(first, second);

            gate.SetResult(null); // 放行门控，触发 reload 重新拉取
            await first;

            // 最新请求胜出：最终展示 data2。
            Assert.Single(records);
            Assert.Same(data2, records[0]);
            Assert.Equal("新数据", records[0].PeerName);
            // 共调用 GetRecords 两次（首次 + reload）。
            Assert.Equal(2, fake.GetRecordsCalls);
            return Unit.Default;
        });
    }

    [Fact]
    public async Task LoadRecordsAsync_StorageThrows_PublishesError()
    {
        await RunOnDispatcherAsync(async () =>
        {
            var fake = new FakeStorageService();
            fake.GetRecordsThrows = new InvalidOperationException("存储内部异常");

            var records = new ObservableCollection<TransferRecord>();
            var logs = new List<(string Level, string Message)>();
            var controller = new RecordsPaneController(fake, Dispatcher.CurrentDispatcher, records,
                _ => { }, (lvl, msg) => logs.Add((lvl, msg)));

            // 存储抛异常：LoadRecordsAsync 不向外抛出，改为发布错误日志。
            await controller.LoadRecordsAsync();

            // 断言日志回调收到 ERROR 级别、消息含「加载记录失败」。
            var errorLog = Assert.Single(logs);
            Assert.Equal(TransferConst.LogError, errorLog.Level);
            Assert.Contains("加载记录失败", errorLog.Message);
            // Records 保持为空。
            Assert.Empty(records);
            return Unit.Default;
        });
    }

    /// <summary>
    /// 经典 WPF 单测模式：在 STA 线程上创建 Dispatcher、安装 DispatcherSynchronizationContext、
    /// 以 InvokeAsync 调度 action 并 Dispatcher.Run 泵消息，从而让被调用的 await 延续回到
    /// STA 线程（_dispatcher.Invoke 与集合操作安全），避免无泵 Dispatcher 引发的死锁。
    /// </summary>
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

    // 空标记类型，避免非空返回值测试改动 helper 签名。
    private record struct Unit
    {
        public static Unit Default => default;
    }
}

/// <summary>
/// IStorageService 的内存 fake：供 RecordsPaneControllerTests / DevicePaneControllerTests 共用。
/// 记录 GetRecords 调用次数、支持按次返回不同数据集（GetRecordsSequence 队列）、记录
/// UpsertDevice / SetDeviceFavorite 调用参数；GetStats / ClearRecords 简单实现即可。
/// </summary>
internal sealed class FakeStorageService : IStorageService
{
    public List<TransferRecord> Records { get; } = new();

    public List<DeviceInfo> Devices { get; } = new();

    /// <summary>按次返回不同数据集：队首非空时将逐个出队作为本次 GetRecords 的返回值。</summary>
    public Queue<Func<List<TransferRecord>>>? GetRecordsSequence { get; set; }

    /// <summary>非空时 GetRecords 直接抛此异常（用于异常路径测试）。</summary>
    public Exception? GetRecordsThrows { get; set; }

    public int GetRecordsCalls { get; private set; }

    public int ClearRecordsCalls { get; private set; }

    public List<DeviceInfo> UpsertedDevices { get; } = new();

    public List<(string Addr, bool Favorite)> SetFavoriteCalls { get; } = new();

    public List<TransferRecord> GetRecords(string? direction = null, string? type = null,
        string? peerAddr = null, string? status = null, DateTime? from = null, DateTime? to = null,
        string? search = null, string orderBy = "created_at DESC", int limit = 200)
    {
        GetRecordsCalls++;
        if (GetRecordsThrows != null) throw GetRecordsThrows;
        if (GetRecordsSequence is { Count: > 0 })
            return GetRecordsSequence.Dequeue()();
        return new List<TransferRecord>(Records);
    }

    public List<DeviceInfo> GetDevices() => new(Devices);

    public void UpsertDevice(DeviceInfo device) => UpsertedDevices.Add(device);

    public bool SetDeviceFavorite(string addr, bool favorite)
    {
        SetFavoriteCalls.Add((addr, favorite));
        return true;
    }

    public (long totalCount, long totalBytes, long sendCount, long recvCount) GetStats()
        => (0, 0, 0, 0);

    public int ClearRecords()
    {
        ClearRecordsCalls++;
        return 0;
    }
}
