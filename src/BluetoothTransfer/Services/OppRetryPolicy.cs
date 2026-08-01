namespace BluetoothTransfer.Services;

/// <summary>
/// OPP 推送失败重试策略：对可重试的临时失败（连接失败/超时/对端忙 0xC3/断开）
/// 按指数退避自动重试；不可重试的错误（未配对/无 OPP 能力/用户取消/本地文件问题）直接放弃。
/// 纯逻辑实现，无蓝牙依赖，可离线单测。
/// </summary>
public sealed class OppRetryPolicy
{
    /// <summary>总允许尝试次数（1 次初始 + 配置的重试次数）。</summary>
    public int MaxAttempts { get; }

    /// <summary>基础退避间隔（秒），第 N 次重试等待 baseDelay * N。</summary>
    public int BaseDelaySeconds { get; }

    public OppRetryPolicy(int retryCount, int baseDelaySeconds)
    {
        MaxAttempts = Math.Max(1, retryCount + 1);
        BaseDelaySeconds = Math.Max(1, baseDelaySeconds);
    }

    /// <summary>
    /// 判断是否应重试。<paramref name="attemptIndex"/> 为从 1 开始的尝试序号
    /// （第一次尝试为 1）；返回 true 表示还可重试。
    /// </summary>
    public bool ShouldRetry(int attemptIndex, string? errorMessage)
        => attemptIndex < MaxAttempts && IsRetryableError(errorMessage);

    /// <summary>第 <paramref name="attemptIndex"/> 次失败后的等待时长（1-based）。</summary>
    public TimeSpan NextDelay(int attemptIndex)
        => TimeSpan.FromSeconds(BaseDelaySeconds * Math.Max(1, attemptIndex));

    /// <summary>错误是否属于可重试的临时失败。</summary>
    public static bool IsRetryableError(string? message)
    {
        if (string.IsNullOrEmpty(message)) return true;
        var text = message;
        // 可重试：连接失败/中断、超时、对端忙（Forbidden 0xC3）、对端提前关闭、设备未就绪
        if (text.Contains("0xC3", StringComparison.OrdinalIgnoreCase)) return true;
        if (text.Contains("提前关闭", StringComparison.OrdinalIgnoreCase)) return true;
        if (text.Contains("连接", StringComparison.OrdinalIgnoreCase)) return true;
        if (text.Contains("超时", StringComparison.OrdinalIgnoreCase)) return true;
        if (text.Contains("设备未就绪", StringComparison.OrdinalIgnoreCase)) return true;
        if (text.Contains("中止", StringComparison.OrdinalIgnoreCase)) return true;
        // 不可重试：用户取消、未配对、无 OPP 能力、文件/本地问题
        if (text.Contains("取消", StringComparison.OrdinalIgnoreCase)) return false;
        if (text.Contains("未配对", StringComparison.OrdinalIgnoreCase)) return false;
        if (text.Contains("不支持", StringComparison.OrdinalIgnoreCase)) return false;
        if (text.Contains("未声明 OPP", StringComparison.OrdinalIgnoreCase)) return false;
        if (text.Contains("不存在", StringComparison.OrdinalIgnoreCase)) return false;
        if (text.Contains("超过 4GB", StringComparison.OrdinalIgnoreCase)) return false;
        return false;
    }
}
