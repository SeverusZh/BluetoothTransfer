using System.Collections.ObjectModel;
using System.Windows;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.ViewModels;

/// <summary>
/// 记录窗格控制器：承载传输记录加载/导出/清空与统计相关（LoadRecordsAsync、ExportRecordsAsync、
/// ClearRecordsAsync、GetStats 相关）的纯内部逻辑。
/// 它为 <see cref="OppViewModel"/> 的一窗格职责做组合式拆分，避免"上帝 ViewModel"继续膨胀。
///
/// 约束与设计：
/// 1. Records 集合对象本身仍由 ViewModel 暴露（XAML 绑定），本控制器只通过构造注入的集合引用做内部操作；
/// 2. 不反向依赖 ViewModel：回写状态栏或发布日志一律经构造注入的回调委托；
/// 3. LoadRecordsAsync 带并发保护：加载与清空、多路触发（接收完成/发送结束）可能并发，
///    用"最新请求胜出"的串行化守卫保证最终展示的总是最近一次请求的数据，避免竞态导致旧数据回写。
///    该守卫只改变竞态时序，不改变任何单次请求的展示文案与语义。
/// </summary>
internal sealed class RecordsPaneController
{
    private readonly StorageService _storage;
    private readonly Dispatcher _dispatcher;
    private readonly ObservableCollection<TransferRecord> _records;
    private readonly Action<string> _setStatus;
    private readonly Action<string, string> _publishLog;

    // 并发保护状态：_loadInProgress 表示已有一次加载在途；期间新请求置 _reloadRequested，
    // 待这次加载完成后再重新拉取一次，保证最终展示的是最近一次请求的数据（最新请求胜出）。
    private readonly object _loadGate = new();
    private Task? _loadTask;
    private bool _loadInProgress;
    private bool _reloadRequested;

    public RecordsPaneController(
        StorageService storage,
        Dispatcher dispatcher,
        ObservableCollection<TransferRecord> records,
        Action<string> setStatus,
        Action<string, string> publishLog)
    {
        _storage = storage;
        _dispatcher = dispatcher;
        _records = records;
        _setStatus = setStatus;
        _publishLog = publishLog;
    }

    public Task LoadRecordsAsync()
    {
        lock (_loadGate)
        {
            if (_loadInProgress)
            {
                // 已有加载在途：记录"再刷新一次"的请求，复用同一底层任务，避免并发清空/回写竞态。
                _reloadRequested = true;
                return _loadTask!;
            }
            _loadInProgress = true;
            _loadTask = LoadCoreAsync();
            return _loadTask;
        }
    }

    private async Task LoadCoreAsync()
    {
        try
        {
            do
            {
                _reloadRequested = false;
                var records = await Task.Run(() => _storage.GetRecords(limit: 200));
                // 若在后台加载期间又有新请求到达，则忽略本次结果重新拉取，保证最终显示最新数据。
                if (_reloadRequested) continue;
                _dispatcher.Invoke(() =>
                {
                    _records.Clear();
                    foreach (var r in records) _records.Add(r);
                });
            } while (_reloadRequested);
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogError, $"加载记录失败：{ex.Message}");
        }
        finally
        {
            lock (_loadGate)
            {
                _loadInProgress = false;
                _loadTask = null;
            }
        }
    }

    // 导出/清空涉及存储查询与磁盘写入：后台执行，避免阻塞 UI 线程。
    // 命令经 SafeAsync 启动，方法内 await 未 ConfigureAwait(false)，延续自然回到 UI 线程，
    // 因此状态栏/日志更新仍发生在 UI 线程。
    public async Task ExportRecordsAsync(string filter, string ext, Func<List<TransferRecord>, string, string> export)
    {
        try
        {
            var dialog = new Microsoft.Win32.SaveFileDialog
            {
                Filter = filter,
                FileName = $"transfer_records_{DateTime.Now:yyyyMMdd_HHmmss}.{ext}"
            };
            if (dialog.ShowDialog() != true) return;
            var count = await Task.Run(() =>
            {
                var records = _storage.GetRecords(limit: 10000);
                export(records, dialog.FileName);
                return records.Count;
            });
            _setStatus($"已导出 {count} 条记录到 {ext.ToUpperInvariant()}");
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogError, $"导出失败：{ex.Message}");
            _setStatus($"导出失败：{ex.Message}");
        }
    }

    public async Task ClearRecordsAsync()
    {
        long total;
        try
        {
            total = await Task.Run(() => _storage.GetStats().totalCount);
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogError, $"清空传输记录失败：{ex.Message}");
            _setStatus($"清空失败：{ex.Message}");
            return;
        }
        if (total == 0)
        {
            _setStatus("没有可清空的记录");
            return;
        }
        var confirm = MessageBox.Show(
            $"确定要清空全部 {total} 条传输记录吗？该操作不可恢复。",
            "清空传输记录",
            MessageBoxButton.YesNo,
            MessageBoxImage.Warning);
        if (confirm != MessageBoxResult.Yes) return;

        try
        {
            var deleted = await Task.Run(() => _storage.ClearRecords());
            _records.Clear();
            _setStatus($"已清空 {deleted} 条传输记录");
        }
        catch (Exception ex)
        {
            _publishLog(TransferConst.LogError, $"清空传输记录失败：{ex.Message}");
            _setStatus($"清空失败：{ex.Message}");
        }
    }
}
