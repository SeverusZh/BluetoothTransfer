using System.Runtime.InteropServices;
using System.IO;
using BluetoothTransfer.Receiver;

var command = args.Length == 0 ? "gui" : args[0].ToLowerInvariant();
#if CLI_ONLY
// 纯 CLI 形态没有 GUI 壳：无参数/双击直接进入 run（接收监听）
if (command is "gui" or "--gui") command = "run";
#endif

// 单实例守卫只覆盖实际长期占用 RFCOMM 接收通道的模式。
// 注意 CLI_ONLY 形态：command 虽被改写为 run，但 ReceiverCli.RunAsync 仍按原始 args 派发，
// 无参 / gui 实际走的是一次性命令路径（既有行为，本次不改派发逻辑），因此只对显式 run 占守卫。
#if CLI_ONLY
var isLongRunningMode = args.Length > 0 && args[0].ToLowerInvariant() == "run";
#else
var isLongRunningMode = command is "gui" or "--gui" or "run";
#endif

const int ExitCodeAlreadyRunning = 3;
// 单实例锁因权限不足无法获取（典型：已有管理员权限的 btrecv 实例运行）单独用 4 区分，
// 避免与“正常重复启动”混淆，便于脚本/用户判断该以相同权限重试。
const int ExitCodePermissionDenied = 4;
// btrecv 单实例名称：gui 与 run 共用同一个名字，因此两种长驻形态也互斥。
const string SingleInstanceName = "BluetoothTransfer.btrecv.Receiver";

if (isLongRunningMode)
{
    SingleInstanceMutex? singleInstance;
    try
    {
        singleInstance = SingleInstanceMutex.TryAcquire(SingleInstanceName);
    }
    catch (UnauthorizedAccessException ex)
    {
        // 混合权限场景：已有 btrecv 以管理员权限运行时，普通权限进程打开其命名 Mutex 可能被对象 ACL 拒绝。
        // 与“正常重复启动”原因不同，单独提示并用退出码 4 区分；只捕获该异常，其它异常照常上抛。
        ReportGuardPermissionDenied(command, ex);
        return ExitCodePermissionDenied;
    }

    if (singleInstance == null)
    {
        ReportAlreadyRunning(command);
        return ExitCodeAlreadyRunning;
    }

    using (singleInstance)
    {
#if !CLI_ONLY
        if (command is "gui" or "--gui")
        {
            HideConsoleWindow();
            var app = new System.Windows.Application();
            var saveDir = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                "Downloads", "BluetoothReceive");
            app.Run(new ReceiverWindow(saveDir, ask: true));
            return 0;
        }
#endif
        return await ReceiverCli.RunAsync(args);
    }
}

// 一次性命令（help / version / 未知参数）：保持原有行为，不占用单实例守卫。
return await ReceiverCli.RunAsync(args);

// 重复启动提示：GUI 形态没有可读控制台，用对话框提示；CLI 形态写 stderr（中文提示）。
// 两条路径都由调用方返回非 0 退出码。
static void ReportAlreadyRunning(string command)
{
#if !CLI_ONLY
    if (command is "gui" or "--gui")
    {
        HideConsoleWindow();
        System.Windows.MessageBox.Show(
            "btrecv 已在运行，请先关闭已有的接收窗口。\n（同一时刻只允许一个接收实例）",
            "蓝牙接收助手",
            System.Windows.MessageBoxButton.OK,
            System.Windows.MessageBoxImage.Information);
        return;
    }
#endif
    Console.Error.WriteLine("[ERROR] btrecv 已在运行：同一时刻只允许一个接收实例，请先关闭已有窗口/进程。");
}

// 单实例锁权限不足提示（混合权限启动）：GUI 形态用对话框，CLI 形态写 stderr；退出码 4。
// 只捕获 UnauthorizedAccessException 时调用，其它异常不经过这里。
static void ReportGuardPermissionDenied(string command, UnauthorizedAccessException ex)
{
    const string message = "无法获取接收助手的单实例锁：已有实例可能以更高权限运行。请以相同权限重试。";
#if !CLI_ONLY
    if (command is "gui" or "--gui")
    {
        HideConsoleWindow();
        System.Windows.MessageBox.Show(
            $"{message}\n\n（详细信息：{ex.Message}）",
            "蓝牙接收助手",
            System.Windows.MessageBoxButton.OK,
            System.Windows.MessageBoxImage.Warning);
        return;
    }
#endif
    Console.Error.WriteLine($"[ERROR] {message}（{ex.Message}）");
}

#if !CLI_ONLY
[DllImport("user32.dll")]
static extern IntPtr GetConsoleWindow();

[DllImport("user32.dll")]
static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

static void HideConsoleWindow()
{
    try { ShowWindow(GetConsoleWindow(), 0); } catch { }
}
#endif

/// <summary>
/// btrecv 单实例守卫（内联实现）：Receiver 工程不引用 AppServices（避免把 WPF 依赖带进 CLI_ONLY 裁剪形态），
/// 因此只用 <see cref="Mutex"/> 完成最小闭环；重复启动直接提示退出，不需要“激活已有窗口”的通知。
///
/// 线程约束：Mutex 所有权属于创建线程；Dispose 只在创建线程调用 <see cref="Mutex.ReleaseMutex"/>，
/// 其他线程只关闭句柄（此时进程即将退出，由系统回收），避免跨线程释放抛 <see cref="ApplicationException"/>。
/// </summary>
internal sealed class SingleInstanceMutex : IDisposable
{
    private const string WindowsLocalPrefix = @"Local\";

    private readonly Mutex _mutex;
    private readonly int _ownerThreadId;
    private bool _disposed;

    private SingleInstanceMutex(Mutex mutex)
    {
        _mutex = mutex;
        _ownerThreadId = Environment.CurrentManagedThreadId;
    }

    /// <summary>尝试成为主实例；返回 null 表示已有 btrecv 进程在运行。</summary>
    public static SingleInstanceMutex? TryAcquire(string name)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(name);
        if (name.Contains('\\'))
            throw new ArgumentException("单实例名称不能包含反斜杠（会与命名空间前缀 Local\\ 冲突）", nameof(name));

        // Windows 用 Local\ 前缀做会话级隔离；其他平台（如交叉编译后的单测）使用原生名称。
        var mutexName = OperatingSystem.IsWindows() ? WindowsLocalPrefix + name : name;
        var mutex = new Mutex(initiallyOwned: false, mutexName);
        try
        {
            bool acquired;
            try
            {
                acquired = mutex.WaitOne(0);
            }
            catch (AbandonedMutexException)
            {
                // 上一个 btrecv 异常退出（未释放 Mutex）：系统已把所有权交给本次等待，直接接管。
                acquired = true;
            }

            if (!acquired)
            {
                mutex.Dispose();
                return null;
            }
            return new SingleInstanceMutex(mutex);
        }
        catch
        {
            mutex.Dispose();
            throw;
        }
    }

    /// <summary>释放 Mutex；可重复调用。</summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try
        {
            if (Environment.CurrentManagedThreadId == _ownerThreadId)
                _mutex.ReleaseMutex();
        }
        finally
        {
            _mutex.Dispose();
        }
    }
}
