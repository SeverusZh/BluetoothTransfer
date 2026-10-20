using System.Diagnostics;
using System.Windows;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer;

/// <summary>
/// Interaction logic for App.xaml
/// </summary>
public partial class App : Application
{
    /// <summary>
    /// 单实例标识：同一登录会话内用该名字的命名内核对象做互斥。
    /// 说明：跨版本升级必须保持同名，否则新旧版本可同时启动；改名需评估兼容性。
    /// </summary>
    private const string SingleInstanceAppId = "BluetoothTransfer.Gui";

    private SingleInstanceGuard? _instanceGuard;
    private MainWindow? _mainWindow;

    protected override void OnStartup(StartupEventArgs e)
    {
        // 资源管理器右键菜单 / 「发送到」入口：--send <path...> / --send-text <file>。
        // 解析必须早于建窗与守卫：非主实例只转发载荷、不建窗；无参数/未知参数解析为空请求，走正常启动。
        var ownSendRequest = ShellSendRequest.Parse(e.Args);

        // 先建守卫再建窗：二次启动绝不创建窗口，只向已运行实例发激活信号，然后以退出码 0 结束。
        // 混合权限（管理员实例 vs 普通权限二次启动）下打开命名内核对象可能被 ACL 拒绝，统一在工厂里提示退出。
        _instanceGuard = CreateInstanceGuardOrExit();
        if (_instanceGuard == null) return;

        var signaledExistingInstance = false;
        if (!_instanceGuard.IsPrimaryInstance)
        {
            // 顺序不能反：先把载荷写入收件箱，再发激活信号——主实例收到信号后会立即排空收件箱，
            // 若先发信号后写文件，主实例可能在文件落盘前完成排空，导致本次右键发送静默丢失。
            var inbox = new ExternalSendInbox();
            var payloadForwarded = true;
            var payloadError = "";
            // 有发送项、或只有解析提示（例如选中路径已被移动/删除）时都转发：后者让主实例状态栏能看到失败原因，
            // 避免“右键后主窗口被激活但没有任何提示”的静默失败。
            if (ownSendRequest.HasItems || ownSendRequest.Warnings.Count > 0)
                payloadForwarded = inbox.TryWrite(ownSendRequest, out payloadError);
            if (!payloadForwarded)
            {
                payloadError = string.IsNullOrWhiteSpace(payloadError) ? "未知原因" : payloadError;
                Trace.WriteLine($"[App] 发送请求写入收件箱失败：{payloadError}");
            }

            // 主实例可能仍在启动（先占 Mutex、后建激活事件），TrySignalExisting 内部会短间隔重试。
            signaledExistingInstance = SingleInstanceGuard.TrySignalExisting(SingleInstanceAppId);
            if (!signaledExistingInstance)
            {
                // 没找到可通知的实例：多半是旧实例正在退出（Mutex 即将释放）。释放句柄后重试一次接管，
                // 避免“旧实例已退出、新实例也退出”的启动空窗；仍抢不到则按二次启动处理。
                _instanceGuard.Dispose();
                _instanceGuard = CreateInstanceGuardOrExit();
                if (_instanceGuard == null) return;
            }
            if (!_instanceGuard.IsPrimaryInstance)
            {
                if (!payloadForwarded)
                {
                    // 载荷转发失败但主实例在运行：不能静默退出，否则用户右键后没有任何反应。
                    MessageBox.Show(
                        $"无法把待发送的文件转交给已运行的蓝牙传输窗口：\n{payloadError}\n" +
                        "请重试，或把文件/文件夹直接拖到主窗口中发送。",
                        "蓝牙传输",
                        MessageBoxButton.OK,
                        MessageBoxImage.Warning);
                }
                else if (!signaledExistingInstance)
                {
                    // 极低概率路径：既通知不到已有实例、也抢不到 Mutex（旧实例正在退出的窗口期）。
                    // 载荷已留在收件箱，下次主实例启动/激活时会兜底消费，这里只留诊断日志、不弹窗打扰用户。
                    Trace.WriteLine("[App] 二次启动未能通知已有实例，且旧实例尚未释放 Mutex；发送请求已留在收件箱，本次静默退出。");
                }
                _instanceGuard.Dispose();
                _instanceGuard = null;
                Shutdown(payloadForwarded ? 0 : 1);
                return;
            }
            // 走到这里说明“旧实例正在退出”窗口期内接管成功：本进程成为主实例，
            // ownSendRequest 由下面的正常启动路径消费，不经过收件箱也不会丢。
        }

        base.OnStartup(e);

        // App.xaml 已移除 StartupUri，主窗口在守卫判定为主实例后手动创建并显示；
        // 同时设置 Application.MainWindow，保持与 WPF 约定一致（Owner / 任务栏行为依赖它）。
        _mainWindow = new MainWindow();
        _mainWindow.Closed += (_, _) => _mainWindow = null;
        MainWindow = _mainWindow;
        _mainWindow.Show();

        // 主实例：消费自身命令行载荷 + 排空收件箱（含上次崩溃/信号丢失的残留），全部在 UI 线程由窗口处理。
        // 内部已捕获并上报异常，不阻塞启动；无载荷时该调用会很快返回。
        _ = _mainWindow.HandleExternalSendRequestsAsync(ownSendRequest);

        // 不阻塞 UI 启动：后台恢复上次未完成的助手推送（durable send journal）。
        _ = Task.Run(ResumePendingSendsAsync);
    }

    /// <summary>
    /// 创建单实例守卫；若因权限不足无法打开命名内核对象（典型：主实例以管理员权限运行、
    /// 本次以普通权限启动，对象 ACL 拒绝访问），弹中文提示并以退出码 1 结束启动，返回 null。
    /// 只捕获 <see cref="UnauthorizedAccessException"/>，其它异常保持原样上抛，不静默吞掉。
    /// </summary>
    private SingleInstanceGuard? CreateInstanceGuardOrExit()
    {
        try
        {
            return new SingleInstanceGuard(SingleInstanceAppId, RequestActivationFromSecondInstance);
        }
        catch (UnauthorizedAccessException ex)
        {
            // 此时无法安全判定主/次实例，也不应继续建窗（会与已有实例抢 RFCOMM/SQLite），提示后非 0 退出。
            Trace.WriteLine($"[App] 单实例守卫初始化失败（权限不足）：{ex}");
            MessageBox.Show(
                "检测到另一个蓝牙传输实例正在以更高权限运行。\n请先退出该实例，或以相同权限启动。",
                "蓝牙传输",
                MessageBoxButton.OK,
                MessageBoxImage.Warning);
            Shutdown(1);
            return null;
        }
    }

    /// <summary>
    /// 二次启动激活回调：由 <see cref="SingleInstanceGuard"/> 在线程池线程触发，
    /// 这里切回 UI 线程再操作窗口；排队而非阻塞，保证二次启动进程能立即退出。
    /// </summary>
    private void RequestActivationFromSecondInstance()
    {
        var dispatcher = Dispatcher;
        if (dispatcher.HasShutdownStarted) return;
        try
        {
            dispatcher.BeginInvoke(new Action(() =>
            {
                // 窗口真正关闭（托盘退出）后 _mainWindow 会被置空：此时激活请求已无意义，直接忽略。
                var window = _mainWindow;
                if (window == null) return;
                // 先显示/激活窗口，再排空收件箱：二次实例的载荷（--send 路径）已写入 inbox，
                // 由窗口用 ~400ms 去抖合并多选时“逐项拉起多个进程”的情况后再统一入队。
                window.ActivateFromSecondInstance();
                window.ScheduleExternalInboxDrain();
            }));
        }
        catch (InvalidOperationException)
        {
            // Dispatcher 正在关闭（应用退出中）时排队会抛异常：丢掉这次迟到的激活请求即可。
        }
    }

    protected override void OnExit(ExitEventArgs e)
    {
        // 正常退出释放 Mutex 与激活事件，保证下一次启动可以立即成为主实例。
        _instanceGuard?.Dispose();
        _instanceGuard = null;
        base.OnExit(e);
    }

    /// <summary>
    /// 启动恢复：读 journal 中未完成的助手通道传输，按原文件名 + 原 SHA-256 重新发起，
    /// 复用既有 HELLO/OFFER(ResumeOffset) 续传协议从接收端已确认偏移继续。
    /// 说明：GUI 的 ViewModel 私有持有发送队列（本次不改 ViewModels），因此这里用独立服务实例直连重发；
    /// 同一 journal 的并发保护由租约（TransferJournal.Begin/TryClaim）保证，不会与 UI 手动重发双开。
    /// </summary>
    private static async Task ResumePendingSendsAsync()
    {
        try
        {
            var events = new EventBus();
            var config = AppConfig.Load();
            var storage = new StorageService();
            var discovery = new OppDiscoveryService(events);
            var packages = new PendingPackageStore();
            var journal = new TransferJournal(
                new FileTransferJournalStore(FileTransferJournalStore.DefaultPath), packages: packages);
            var push = new AssistantPushService(events, storage, config, discovery, journal, packages);
            var resume = new SendResumeService(journal, push, events, packages, (entry, reason) =>
                storage.AddRecord(new TransferRecord
                {
                    Direction = TransferConst.DirSend,
                    Type = TransferConst.TypeFile,
                    PeerName = entry.PeerName,
                    PeerAddr = entry.PeerAddr,
                    Name = entry.DisplayName,
                    Size = entry.Size,
                    Status = TransferConst.StatusFailed,
                    Channel = TransferConst.ChannelAssistant,
                    Note = $"断点续传已放弃：{reason}"
                }));
            await resume.ResumeAllAsync();
        }
        catch
        {
            // 启动恢复失败不影响应用启动；残留条目由 TTL（7 天）清理
        }
    }
}
