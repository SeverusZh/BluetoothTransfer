using System.ComponentModel;
using System.Diagnostics;
using System.Threading;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Hardcodet.Wpf.TaskbarNotification;

namespace BluetoothTransfer;

public partial class MainWindow : Window
{
    /// <summary>
    /// 外部发送（资源管理器右键 / 「发送到」）的去抖间隔：多选时 Shell 可能逐项拉起多个二次进程，
    /// 每个进程写一份收件箱 JSON 并发一次激活信号；每次激活都重置计时，静默满 400ms 后才统一排空合并。
    /// </summary>
    private static readonly TimeSpan ExternalInboxDebounce = TimeSpan.FromMilliseconds(400);

    private bool _forceClose;
    private TaskbarIcon? _trayIcon;

    /// <summary>二次实例 → 主实例的发送载荷收件箱（默认 %LOCALAPPDATA%\BluetoothTransfer\inbox）。</summary>
    private readonly ExternalSendInbox _externalInbox = new();

    /// <summary>
    /// 收件箱排空串行化：启动排空与激活去抖排空可能相邻触发，避免同一份 JSON 被两个排空流程重复入队。
    /// 排空本身在后台线程做文件 IO，这里只保证「读出 + 删除」不并发。
    /// </summary>
    private readonly SemaphoreSlim _externalDrainGate = new(1, 1);

    /// <summary>激活去抖计时器：每次收到二次实例激活信号都重置，最后一批载荷到齐后再排空。</summary>
    private DispatcherTimer? _externalDrainTimer;

    public MainWindow()
    {
        InitializeComponent();
        AttachLogAutoscroll();
        var version = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
        Title = $"蓝牙传输 v{version?.ToString(3) ?? "1.4.0"}";
        Loaded += (_, _) =>
        {
            _trayIcon = (TaskbarIcon)FindResource("TrayIcon");
            try
            {
                using var stream = Application.GetResourceStream(
                    new Uri("pack://application:,,,/Assets/app.ico", UriKind.Absolute))?.Stream;
                if (stream != null)
                    _trayIcon.Icon = new System.Drawing.Icon(stream);
            }
            catch (Exception ex)
            {
                // 托盘图标加载失败不影响主界面，仅记录日志
                var vm = (ViewModels.OppViewModel)DataContext;
                vm.PublishLog("WARN", $"托盘图标加载失败：{ex.Message}");
            }
        };
    }

    private ScrollViewer? _logScroller;
    private bool _followLog = true;

    private void AttachLogAutoscroll()
    {
        LogListBox.Loaded += (_, _) =>
        {
            _logScroller = Behaviors.HoverScrollBehavior.FindScrollViewer(LogListBox);
            if (_logScroller == null) return;
            _logScroller.ScrollChanged += (_, e) =>
            {
                var atBottom = e.VerticalOffset >= e.ExtentHeight - e.ViewportHeight - 1;
                if (atBottom)
                    _followLog = true;
                else if (e.VerticalChange < 0)
                    _followLog = false;
            };
        };
        var vm = (ViewModels.OppViewModel)DataContext;
        vm.LogLines.CollectionChanged += (_, _) =>
        {
            if (_followLog && _logScroller != null)
                _logScroller.ScrollToEnd();
        };
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        e.Effects = e.Data.GetDataPresent(DataFormats.FileDrop)
            ? DragDropEffects.Copy
            : DragDropEffects.None;
        e.Handled = true;
    }

    private async void OnFileDrop(object sender, DragEventArgs e)
    {
        if (!e.Data.GetDataPresent(DataFormats.FileDrop)) return;
        var files = (string[])e.Data.GetData(DataFormats.FileDrop);
        if (files.Length == 0) return;

        var vm = (ViewModels.OppViewModel)DataContext;
        await vm.SendDroppedAsync(files);
    }

    private void OnClosing(object? sender, CancelEventArgs e)
    {
        if (!_forceClose)
        {
            e.Cancel = true;
            Hide();
            _trayIcon?.ShowBalloonTip("蓝牙传输", "已最小化到系统托盘", BalloonIcon.Info);
        }
    }

    private void OnTrayDoubleClick(object sender, RoutedEventArgs e)
    {
        Show();
        WindowState = WindowState.Normal;
        Activate();
    }

    private void OnTrayShow(object sender, RoutedEventArgs e)
    {
        Show();
        WindowState = WindowState.Normal;
        Activate();
    }

    /// <summary>
    /// 二次启动激活：把窗口从托盘隐藏 / 最小化状态恢复并前台化。
    /// 由 App 在收到第二实例的激活信号后经 Dispatcher 调用（必须运行在 UI 线程）。
    /// 只做显示与激活，不触碰托盘/关闭逻辑：_forceClose 与 OnClosing 的最小化到托盘语义保持不变。
    /// </summary>
    public void ActivateFromSecondInstance()
    {
        Show();
        WindowState = WindowState.Normal;
        // 先置顶再取消：绕过 Windows 对非前台进程 SetForegroundWindow 的调用限制，保证窗口真正到前台。
        Topmost = true;
        Topmost = false;
        Activate();
    }

    // ------------------------------------------------------------------ 外部发送（资源管理器右键 / 发送到 / 二次实例转发）

    /// <summary>
    /// 主实例启动时调用（UI 线程）：先消费自身命令行载荷（<c>--send</c>/<c>--send-text</c>），
    /// 再排空收件箱（含上次崩溃/信号丢失的残留），合并去重后交给 ViewModel 进入「待发送」队列。
    /// 所有异常都在内部转为中文日志/状态栏提示，不让启动线程抛出。
    /// </summary>
    public async Task HandleExternalSendRequestsAsync(ShellSendRequest ownRequest)
    {
        // 日志回调必须在 UI 线程创建：先取到 ViewModel 引用，后台排空线程只经 EventBus 发布日志
        // （EventBus/OnLog 内部会切回 UI 线程），不直接访问 DataContext 这类有线程亲和的 WPF 对象。
        var log = CreateExternalSendWarningLogger();
        try
        {
            // 1) 过期残留清理：崩溃中断的 .tmp 与超过 7 天未被消费的 JSON（文件 IO 放后台线程，避免拖慢启动）。
            try
            {
                var cleaned = await Task.Run(() => _externalInbox.CleanResidue(log: log));
                if (cleaned > 0)
                    log($"已清理 {cleaned} 个过期的发送请求残留文件");
            }
            catch (Exception ex)
            {
                log($"清理发送请求残留失败：{ex.Message}");
            }

            // 2) 排空收件箱并与自身参数合并：主实例可能正是在「另一个右键进程写入之后、激活信号到达之前」启动的。
            var inboxRequest = await DrainExternalInboxCoreAsync(log);
            var merged = ShellSendRequest.Merge(new[] { ownRequest, inboxRequest });
            await ApplyExternalSendAsync(merged);
        }
        catch (Exception ex)
        {
            ReportExternalSendFailure($"处理外部发送请求失败：{ex.Message}");
        }
    }

    /// <summary>
    /// 收到二次实例激活信号后调用（UI 线程）：重置 ~400ms 去抖计时器，合并多选时逐项启动的多个进程。
    /// 窗口已由 <see cref="ActivateFromSecondInstance"/> 显示并置前，这里只负责延迟排空收件箱。
    /// </summary>
    public void ScheduleExternalInboxDrain()
    {
        if (_externalDrainTimer == null)
        {
            _externalDrainTimer = new DispatcherTimer(DispatcherPriority.Background, Dispatcher)
            {
                Interval = ExternalInboxDebounce
            };
            _externalDrainTimer.Tick += OnExternalInboxDrainTick;
        }
        // 重置计时：只有最后一次激活后静默满去抖间隔，才认为这一批（多选）载荷已全部落盘。
        _externalDrainTimer.Stop();
        _externalDrainTimer.Start();
    }

    /// <summary>去抖计时器到点：排空收件箱并把新载荷加入待发送队列。</summary>
    private async void OnExternalInboxDrainTick(object? sender, EventArgs e)
    {
        _externalDrainTimer?.Stop();
        var log = CreateExternalSendWarningLogger();
        try
        {
            var request = await DrainExternalInboxCoreAsync(log);
            await ApplyExternalSendAsync(request);
        }
        catch (Exception ex)
        {
            // async void 事件处理器中未捕获的异常会直接终止进程：这里兜底上报，避免一次排空失败打挂应用。
            ReportExternalSendFailure($"读取外部待发送项失败：{ex.Message}");
        }
    }

    /// <summary>串行排空收件箱（启动排空与去抖排空共用；读出后即删除，配合信号量避免重复入队）。</summary>
    private async Task<ShellSendRequest> DrainExternalInboxCoreAsync(Action<string> log)
    {
        await _externalDrainGate.WaitAsync();
        try
        {
            return await _externalInbox.DrainAsync(log);
        }
        finally
        {
            _externalDrainGate.Release();
        }
    }

    /// <summary>把合并后的外部请求交给 ViewModel：激活窗口 → 进入待发送队列（不自动发送，等用户选设备后点按钮）。</summary>
    private async Task ApplyExternalSendAsync(ShellSendRequest request)
    {
        if (DataContext is not ViewModels.OppViewModel vm)
        {
            // 正常生命周期内不会发生（窗口构造即设置 DataContext）；保留诊断，不静默吞掉。
            Trace.WriteLine("[MainWindow] 收到外部发送请求时主界面尚未就绪，请求已丢弃。");
            return;
        }

        if (!request.HasItems)
        {
            // 只有解析提示（如选中路径在启动前被移动/删除）时也必须让用户看到，不静默：
            // 日志页完整记录，状态栏给出最后一条原因（例如「路径不存在或不可访问，已忽略」）。
            foreach (var warning in request.Warnings)
                vm.PublishLog(TransferConst.LogWarn, warning);
            if (request.Warnings.Count > 0)
                vm.StatusText = request.Warnings[request.Warnings.Count - 1];
            return;
        }

        // 显式用户动作（右键 / 发送到）：先显示并置前窗口，再由 VM 提示选择设备。
        ActivateFromSecondInstance();
        await vm.EnqueueExternalSendAsync(request);
    }

    /// <summary>
    /// 创建收件箱日志回调（必须在 UI 线程调用）：先取到 ViewModel 引用再交给后台排空线程使用。
    /// EventBus.Publish → OppViewModel.OnLog 内部会切回 UI 线程，因此后台线程调用 PublishLog 是安全的；
    /// 但 DataContext 是 WPF 依赖属性、有线程亲和，不能在后台线程读取，故在这里提前捕获。
    /// </summary>
    private Action<string> CreateExternalSendWarningLogger()
    {
        var vm = DataContext as ViewModels.OppViewModel;
        return message =>
        {
            if (vm != null)
                vm.PublishLog(TransferConst.LogWarn, message);
            else
                Trace.WriteLine($"[ExternalSendInbox] {message}");
        };
    }

    /// <summary>外部发送流程的兜底失败上报：状态栏 + 日志 + 诊断输出（不静默）。</summary>
    private void ReportExternalSendFailure(string message)
    {
        Trace.WriteLine($"[MainWindow] {message}");
        void Report()
        {
            if (DataContext is ViewModels.OppViewModel vm)
            {
                vm.StatusText = message;
                vm.PublishLog(TransferConst.LogError, message);
            }
        }

        if (Dispatcher.CheckAccess()) Report();
        else Dispatcher.BeginInvoke(new Action(Report));
    }

    // ------------------------------------------------------------------ 资源管理器右键菜单安装/卸载

    private void OnTrayInstallExplorerMenu(object sender, RoutedEventArgs e)
        => ReportExplorerIntegrationResult(ExplorerIntegrationService.Install());

    private void OnTrayUninstallExplorerMenu(object sender, RoutedEventArgs e)
        => ReportExplorerIntegrationResult(ExplorerIntegrationService.Uninstall());

    /// <summary>
    /// 资源管理器集成结果反馈：状态栏 + 日志 + 托盘气球提示三处一致；
    /// 失败时用错误气球与 ERROR 日志（不静默），成功时 Info 气球（Win11 提示文案由服务返回）。
    /// </summary>
    private void ReportExplorerIntegrationResult((bool Success, string Message) result)
    {
        Trace.WriteLine($"[ExplorerIntegration] 成功={result.Success}；{result.Message}");
        if (DataContext is ViewModels.OppViewModel vm)
        {
            vm.StatusText = result.Message;
            vm.PublishLog(result.Success ? TransferConst.LogInfo : TransferConst.LogError, result.Message);
        }
        _trayIcon?.ShowBalloonTip(
            result.Success ? "蓝牙传输" : "蓝牙传输 - 操作失败",
            result.Message,
            result.Success ? BalloonIcon.Info : BalloonIcon.Warning);
    }

    private void OnTrayExit(object sender, RoutedEventArgs e)
    {
        _forceClose = true;
        // 应用真正退出：先停掉去抖计时器（避免退出过程中再触发排空），再清理 ViewModel（退订事件、停止接收监听），最后关闭窗口
        _externalDrainTimer?.Stop();
        (DataContext as IDisposable)?.Dispose();
        _trayIcon?.Dispose();
        Close();
    }
}
