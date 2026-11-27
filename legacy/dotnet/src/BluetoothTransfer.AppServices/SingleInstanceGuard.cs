using System.Diagnostics;

namespace BluetoothTransfer.Services;

/// <summary>
/// 单实例守卫：命名 Mutex 判定“主实例”+ 命名事件把“二次启动 → 激活已有窗口”的请求转给主实例。
///
/// 选型说明：
/// 1) Mutex / 事件名在 Windows 上统一加 <c>Local\</c> 前缀：对象只在当前登录会话内可见，
///    避免不同会话（如 RDP / 服务会话）之间互相误判。
///    产品仅面向 Windows；非 Windows 分支只为可移植性保留（命名对象在 Unix 的实现与行为依平台而定，
///    本机 Linux 上命名 Mutex 可能直接抛 IOException），相关单测只在 Windows 上运行。
/// 2) 主实例创建一次命名事件，并在注入回调时用 <see cref="ThreadPool.RegisterWaitForSingleObject"/> 注册等待；
///    二次启动进程只需打开该事件并 Set，不依赖窗口消息、进程参数或 UIPI，激活路径最短。
/// 3) 二次启动可能早于主实例创建事件（主实例先占 Mutex、后建事件）：<see cref="TrySignalExisting"/>
///    以短间隔重试直到超时（默认 2 秒），超时仍未找到事件才视为“没有可通知的实例”。
///
/// 线程约束（Mutex 所有权属于创建线程）：
/// 本类型按“创建线程持有 Mutex、优先在创建线程 Dispose”的方式使用（WPF 中 OnStartup / OnExit 均在
/// UI 线程）。若 Dispose 发生在其他线程，则只关闭句柄、不调用 <see cref="Mutex.ReleaseMutex"/>，
/// 由进程终止回收，避免跨线程释放抛 <see cref="ApplicationException"/>。
/// 主实例判定用 <c>new Mutex(false, name)</c> + <c>WaitOne(0)</c>，不把构造期所有权当成不可变状态。
/// </summary>
public sealed class SingleInstanceGuard : IDisposable
{
    /// <summary>二次启动等待主实例创建激活事件的最长时间（首实例冷启动可能较慢）。</summary>
    public static readonly TimeSpan DefaultSignalTimeout = TimeSpan.FromSeconds(2);

    private const string WindowsLocalPrefix = @"Local\";
    private const string MutexSuffix = ".SingleInstance.Mutex";
    private const string ActivationEventSuffix = ".SingleInstance.Activate";

    /// <summary>重试打开激活事件的间隔；短间隔保证首实例稍晚建事件时二次启动仍能命中。</summary>
    private static readonly TimeSpan SignalRetryInterval = TimeSpan.FromMilliseconds(100);

    private readonly Mutex _mutex;
    private readonly Action? _onActivationRequested;
    private readonly int _ownerThreadId;
    private EventWaitHandle? _activationEvent;
    private RegisteredWaitHandle? _activationWait;
    private volatile bool _disposed;

    /// <summary>
    /// 创建守卫并立即尝试成为主实例。
    /// </summary>
    /// <param name="appId">
    /// 应用标识（如 <c>"BluetoothTransfer.Gui"</c>）：同一标识的进程互斥。
    /// 测试请使用唯一值（如 GUID）避免用例间互相干扰。
    /// </param>
    /// <param name="onActivationRequested">
    /// 收到二次启动的激活请求时执行的回调（主实例有效）。回调在线程池线程触发，
    /// UI 操作必须由调用方自行切回 UI 线程（WPF 用 Dispatcher）。
    /// </param>
    public SingleInstanceGuard(string appId, Action? onActivationRequested = null)
    {
        ValidateAppId(appId);
        _onActivationRequested = onActivationRequested;
        _ownerThreadId = Environment.CurrentManagedThreadId;
        _mutex = new Mutex(initiallyOwned: false, MutexNameFor(appId));
        try
        {
            IsPrimaryInstance = TryAcquireMutex(_mutex);
            if (!IsPrimaryInstance) return;

            // 主实例总创建激活事件（“事件存在”即代表有实例可被通知，供 TrySignalExisting 判定）；
            // 只有注入回调时才注册线程池等待，避免仅做互斥判定的调用方留下无用的等待注册。
            _activationEvent = new EventWaitHandle(
                false, EventResetMode.AutoReset, ActivationEventNameFor(appId), out _);
            if (onActivationRequested == null) return;

            _activationWait = ThreadPool.RegisterWaitForSingleObject(
                _activationEvent,
                OnActivationSignaled,
                state: null,
                millisecondsTimeOutInterval: Timeout.Infinite,
                executeOnlyOnce: false);
        }
        catch
        {
            // 构造中途失败：先释放已经拿到的内核对象（含已持有的 Mutex）再抛，避免句柄泄漏与实例残留。
            Dispose();
            throw;
        }
    }

    /// <summary>本进程是否为主实例（已持有命名 Mutex）。</summary>
    public bool IsPrimaryInstance { get; }

    /// <summary>
    /// 通知已运行的主实例“有二次启动，请激活窗口”。
    /// </summary>
    /// <returns>
    /// true：已成功向主实例的激活事件发信号；
    /// false：超时仍未找到激活事件，说明当前没有可通知的实例（调用方可按主实例处理或提示用户）。
    /// </returns>
    public static bool TrySignalExisting(string appId, TimeSpan? timeout = null)
    {
        ValidateAppId(appId);
        var eventName = ActivationEventNameFor(appId);
        var waitTimeout = timeout ?? DefaultSignalTimeout;
        if (waitTimeout < TimeSpan.Zero) waitTimeout = TimeSpan.Zero;

        var stopwatch = Stopwatch.StartNew();
        while (true)
        {
            try
            {
                using var activationEvent = EventWaitHandle.OpenExisting(eventName);
                activationEvent.Set();
                return true;
            }
            catch (UnauthorizedAccessException)
            {
                // 事件确实存在，但当前权限打不开（典型：主实例以管理员权限运行，二次启动是普通权限）。
                // 重试不会改变对象的 ACL，直接按“没有可通知的实例”返回 false，避免二次启动因未捕获异常崩溃。
                return false;
            }
            catch (WaitHandleCannotBeOpenedException)
            {
                // 主实例可能仍在启动：先占 Mutex、后建事件。短间隔重试，直到超时仍无事件才返回 false。
                var remaining = waitTimeout - stopwatch.Elapsed;
                if (remaining <= TimeSpan.Zero) return false;
                Thread.Sleep(remaining < SignalRetryInterval ? remaining : SignalRetryInterval);
            }
        }
    }

    /// <summary>
    /// 释放激活等待注册、关闭事件、释放 Mutex（仅在拥有线程调用 ReleaseMutex）。
    /// 可重复调用。
    /// </summary>
    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try
        {
            // 先注销等待再关事件：避免回调在事件句柄关闭后仍被触发。
            _activationWait?.Unregister(null);
            _activationWait = null;
            _activationEvent?.Dispose();
            _activationEvent = null;
            if (IsPrimaryInstance && Environment.CurrentManagedThreadId == _ownerThreadId)
                _mutex.ReleaseMutex();
        }
        finally
        {
            _mutex.Dispose();
        }
    }

    /// <summary>激活等待回调（线程池线程）：只负责调用注入的 Action，异常不允许上抛终止进程。</summary>
    private void OnActivationSignaled(object? state, bool timedOut)
    {
        if (timedOut || _disposed) return;
        try
        {
            _onActivationRequested?.Invoke();
        }
        catch (Exception ex)
        {
            // 线程池回调中抛出的异常会直接终止进程；这里记录到诊断输出，避免一次激活失败打挂整个应用。
            Trace.WriteLine($"[SingleInstanceGuard] 激活回调异常：{ex}");
        }
    }

    /// <summary>尝试立即获取 Mutex；返回 false 表示已有实例占用。</summary>
    private static bool TryAcquireMutex(Mutex mutex)
    {
        try
        {
            return mutex.WaitOne(0);
        }
        catch (AbandonedMutexException)
        {
            // 上一个实例异常退出（未释放 Mutex）：系统已把所有权交给本次等待，直接接管为主实例。
            return true;
        }
    }

    private static void ValidateAppId(string appId)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(appId);
        if (appId.Contains('\\'))
            throw new ArgumentException("appId 不能包含反斜杠（会与命名空间前缀 Local\\ 冲突）", nameof(appId));
    }

    private static string MutexNameFor(string appId) => ScopedName(appId + MutexSuffix);

    private static string ActivationEventNameFor(string appId) => ScopedName(appId + ActivationEventSuffix);

    /// <summary>Windows 加 Local\ 前缀做会话级隔离；其他平台使用原生名称。</summary>
    private static string ScopedName(string name)
        => OperatingSystem.IsWindows() ? WindowsLocalPrefix + name : name;
}
