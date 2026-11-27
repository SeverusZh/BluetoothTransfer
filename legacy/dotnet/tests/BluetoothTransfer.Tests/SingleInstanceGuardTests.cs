using System.Diagnostics;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// <see cref="SingleInstanceGuard"/> 单测。
/// 命名 Mutex / 事件是进程级（Windows 上为会话级）内核对象，因此每个用例都用 Guid 生成唯一 appId，
/// 避免并行用例或残留对象互相干扰；用例不依赖 WPF。
///
/// 运行环境：产品仅面向 Windows；命名内核对象在 Unix 上的 .NET 实现不可靠（本机 Linux 创建命名 Mutex
/// 会直接抛 IOException），所以涉及命名对象的用例标记为仅 Windows 执行，其余平台自动跳过
/// （主测试工程本就 target net8.0-windows，实际由 Windows 真机验证）。
/// </summary>
public class SingleInstanceGuardTests
{
    private static string NewAppId() => "BluetoothTransfer.Tests.SingleInstance." + Guid.NewGuid().ToString("N");

    [WindowsFact]
    public void PrimaryInstance_IsPrimary_AndActivationSignalInvokesCallback()
    {
        var appId = NewAppId();
        using var activated = new ManualResetEventSlim(false);
        using var guard = new SingleInstanceGuard(appId, () => activated.Set());

        Assert.True(guard.IsPrimaryInstance);
        Assert.True(SingleInstanceGuard.TrySignalExisting(appId, TimeSpan.FromSeconds(5)));
        Assert.True(activated.Wait(TimeSpan.FromSeconds(5)), "二次启动信号未触发激活回调");
    }

    [WindowsFact]
    public void SecondaryGuard_OnAnotherThread_IsNotPrimary()
    {
        // 模拟“第二个进程”：Mutex 所有权是线程粒度的，同一线程重复 WaitOne 会因重入而成功，
        // 所以第二实例的判定必须在另一个线程上做。
        var appId = NewAppId();
        using var primary = new SingleInstanceGuard(appId);
        Assert.True(primary.IsPrimaryInstance);

        SingleInstanceGuard? secondary = null;
        var thread = new Thread(() => secondary = new SingleInstanceGuard(appId)) { IsBackground = true };
        thread.Start();
        Assert.True(thread.Join(TimeSpan.FromSeconds(5)), "创建第二实例守卫的线程未在时限内结束");

        Assert.NotNull(secondary);
        try
        {
            Assert.False(secondary!.IsPrimaryInstance);
        }
        finally
        {
            secondary!.Dispose();
        }
    }

    [WindowsFact]
    public void Dispose_ReleasesMutex_SoNextGuardBecomesPrimary()
    {
        var appId = NewAppId();
        var first = new SingleInstanceGuard(appId);
        Assert.True(first.IsPrimaryInstance);
        first.Dispose();

        // 正常退出后 Mutex 应立即释放，下一次启动能立刻成为主实例。
        using var second = new SingleInstanceGuard(appId);
        Assert.True(second.IsPrimaryInstance);
    }

    [WindowsFact]
    public void Dispose_ClosesActivationEvent_SoNoLongerSignalable()
    {
        var appId = NewAppId();
        using var callbackDone = new ManualResetEventSlim(false);
        var guard = new SingleInstanceGuard(appId, () => callbackDone.Set());
        Assert.True(SingleInstanceGuard.TrySignalExisting(appId, TimeSpan.FromSeconds(5)));
        // 先等线程池回调跑完再释放，避免“回调仍在处理中”影响事件句柄的关闭时机。
        Assert.True(callbackDone.Wait(TimeSpan.FromSeconds(5)), "激活回调未在时限内执行");

        guard.Dispose();

        Assert.False(SingleInstanceGuard.TrySignalExisting(appId, TimeSpan.FromMilliseconds(200)));
    }

    [WindowsFact]
    public void TrySignalExisting_ReturnsFalse_WhenNoInstanceRunning()
    {
        var appId = NewAppId();
        var stopwatch = Stopwatch.StartNew();
        var signaled = SingleInstanceGuard.TrySignalExisting(appId, TimeSpan.FromMilliseconds(200));
        stopwatch.Stop();

        Assert.False(signaled);
        // 应重试到超时再返回（主实例可能先占 Mutex、后建事件），而不是立刻失败。
        Assert.True(stopwatch.Elapsed >= TimeSpan.FromMilliseconds(150),
            $"在超时前提前返回：{stopwatch.ElapsedMilliseconds} ms");
    }

    // 以下两个用例只做参数校验，不创建命名内核对象，因此各平台行为一致。
    [Fact]
    public void Constructor_RejectsBlankAppId()
    {
        Assert.Throws<ArgumentException>(() => new SingleInstanceGuard(" "));
    }

    [Fact]
    public void TrySignalExisting_RejectsBlankAppId()
    {
        Assert.Throws<ArgumentException>(() => SingleInstanceGuard.TrySignalExisting(""));
    }
}

/// <summary>仅 Windows 执行的 xUnit Fact：非 Windows 平台自动 Skip（命名内核对象在 Unix 上不可靠）。</summary>
internal sealed class WindowsFactAttribute : FactAttribute
{
    public WindowsFactAttribute()
    {
        if (!OperatingSystem.IsWindows())
            Skip = "命名 Mutex / 命名事件仅在 Windows 上可靠支持";
    }
}
