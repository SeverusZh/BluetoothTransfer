namespace BluetoothTransfer.Core.Discovery;

/// <summary>
/// 助手探测结论三态。必须把"查询失败/未探测"（<see cref="Unknown"/>）与"确认没有助手"（<see cref="Absent"/>）
/// 区分开：SDP 查询会因设备忙/休眠/瞬时异常失败，若把失败当成确定否定结论缓存，
/// 就会出现"对端助手在线但本机识别不到、静默回退 OPP"。
/// </summary>
public enum AssistantProbeOutcome
{
    /// <summary>查询失败或尚未探测：不构成任何确定结论，调用方应重探。</summary>
    Unknown,

    /// <summary>查询成功且确认对端运行接收助手服务。</summary>
    Available,

    /// <summary>查询成功但确认对端没有助手服务（确定否定）。</summary>
    Absent
}

/// <summary>一次助手探测的结果（纯值类型，不依赖 Windows/WPF，可跨平台单测）。</summary>
public readonly record struct AssistantProbeResult(AssistantProbeOutcome Outcome, string? Error = null)
{
    public bool IsAvailable => Outcome == AssistantProbeOutcome.Available;
    public bool IsAbsent => Outcome == AssistantProbeOutcome.Absent;
    public bool IsUnknown => Outcome == AssistantProbeOutcome.Unknown;

    public static readonly AssistantProbeResult Unknown = new(AssistantProbeOutcome.Unknown);
    public static readonly AssistantProbeResult Available = new(AssistantProbeOutcome.Available);
    public static readonly AssistantProbeResult Absent = new(AssistantProbeOutcome.Absent);

    /// <summary>查询失败：结论为 <see cref="AssistantProbeOutcome.Unknown"/>，可携带失败原因。</summary>
    public static AssistantProbeResult Failed(string? error) => new(AssistantProbeOutcome.Unknown, error);

    public override string ToString() => Error is null ? Outcome.ToString() : $"{Outcome}({Error})";
}

/// <summary>
/// 助手在线状态跟踪器（纯类型，不 using Windows / System.Windows，可在 Linux 直接单测）。
///
/// 策略与取舍：
/// 1. 只把正结果（<see cref="AssistantProbeOutcome.Available"/>）作为缓存结论，且带 TTL：过期即视为 Unknown 触发重探。
///    这样"对端助手停止运行"不会被永久相信。
/// 2. 负结果（Absent）与失败（Unknown）都不作为缓存的否定结论：调用方下次查询会拿到 Unknown 并重探。
///    代价是每次发送在无正缓存时多一次 SDP 查询（发送队列每轮只解析一次通道），
///    换来"对端助手后启动后，下一次发送即可发现"，而不是一次失败锁死整个会话。
/// 3. 任一"查询失败"都不得把已知且未过期的 Available 降级——避免瞬时失败导致助手徽标消失/通道退化。
/// 4. 时间戳早于当前结论的过期观察一律忽略——避免 fire-and-forget 的旧 false 覆盖较新的 true。
/// </summary>
public sealed class AssistantPresenceTracker
{
    /// <summary>正结果默认有效期。取较小值以便对端停止助手后能较快回到重探。</summary>
    public static readonly TimeSpan DefaultAvailableTtl = TimeSpan.FromMinutes(2);

    private sealed class Observation
    {
        public AssistantProbeOutcome Outcome;
        public DateTimeOffset ObservedAt;
        public string? Error;
    }

    private readonly Dictionary<string, Observation> _observations = new(StringComparer.OrdinalIgnoreCase);
    private readonly object _gate = new();
    private readonly TimeSpan _availableTtl;
    private readonly Func<DateTimeOffset> _clock;

    /// <param name="availableTtl">正结果有效期，默认 <see cref="DefaultAvailableTtl"/>。</param>
    /// <param name="clock">时间源（测试注入固定时钟）。</param>
    public AssistantPresenceTracker(TimeSpan? availableTtl = null, Func<DateTimeOffset>? clock = null)
    {
        _availableTtl = availableTtl ?? DefaultAvailableTtl;
        _clock = clock ?? (() => DateTimeOffset.UtcNow);
    }

    public TimeSpan AvailableTtl => _availableTtl;

    /// <summary>
    /// 当前可用的缓存结论：只有未过期的正结果返回 <see cref="AssistantProbeOutcome.Available"/>；
    /// 负结果/失败/过期/无记录一律返回 <see cref="AssistantProbeOutcome.Unknown"/>（即需要重探）。
    /// </summary>
    public AssistantProbeResult Current(string? addr)
    {
        if (string.IsNullOrWhiteSpace(addr)) return AssistantProbeResult.Unknown;
        lock (_gate)
        {
            return _observations.TryGetValue(addr, out var o)
                ? Effective(o, _clock())
                : AssistantProbeResult.Unknown;
        }
    }

    /// <summary>
    /// 记录一次探测观察，返回落库后的<b>有效</b>结论。调用方应使用返回值（而非原始结果）决定通道，
    /// 因为失败可能被"不得降级"规则改判为 Available。
    /// </summary>
    /// <param name="observedAt">观察发生时间；默认取当前时钟。早于当前结论的旧观察会被忽略。</param>
    public AssistantProbeResult Record(string? addr, AssistantProbeResult result, DateTimeOffset? observedAt = null)
    {
        if (string.IsNullOrWhiteSpace(addr)) return result;
        var at = observedAt ?? _clock();
        lock (_gate)
        {
            _observations.TryGetValue(addr, out var current);

            // 4) 过期观察不得覆盖更新的结论（fire-and-forget 探测常见）。
            if (current != null && at < current.ObservedAt)
                return Effective(current, _clock());

            switch (result.Outcome)
            {
                case AssistantProbeOutcome.Available:
                case AssistantProbeOutcome.Absent:
                    _observations[addr] = new Observation { Outcome = result.Outcome, ObservedAt = at, Error = result.Error };
                    return result;

                default:
                    // 3) 查询失败：已有未过期正结论时保持 Available，绝不降级。
                    if (current is { Outcome: AssistantProbeOutcome.Available } && at - current.ObservedAt <= _availableTtl)
                        return new AssistantProbeResult(AssistantProbeOutcome.Available, result.Error);
                    // 否则只记一条 Unknown（不构成否定结论，Current 仍返回 Unknown 促重探）。
                    _observations[addr] = new Observation { Outcome = AssistantProbeOutcome.Unknown, ObservedAt = at, Error = result.Error };
                    return result;
            }
        }
    }

    /// <summary>显式失效某地址的缓存（例如用户手动重新扫描）。</summary>
    public void Invalidate(string? addr)
    {
        if (string.IsNullOrWhiteSpace(addr)) return;
        lock (_gate) _observations.Remove(addr);
    }

    public void Clear()
    {
        lock (_gate) _observations.Clear();
    }

    /// <summary>
    /// 最近一次观察到的原始结果（含负结果与失败），仅供诊断/测试；不参与缓存决策。
    /// </summary>
    public AssistantProbeResult LastObserved(string? addr)
    {
        if (string.IsNullOrWhiteSpace(addr)) return AssistantProbeResult.Unknown;
        lock (_gate)
        {
            return _observations.TryGetValue(addr, out var o)
                ? new AssistantProbeResult(o.Outcome, o.Error)
                : AssistantProbeResult.Unknown;
        }
    }

    private AssistantProbeResult Effective(Observation o, DateTimeOffset now)
    {
        // 负结果与失败都不作为确定结论缓存：一律 Unknown，促使调用方重探。
        if (o.Outcome != AssistantProbeOutcome.Available) return AssistantProbeResult.Unknown;
        return now - o.ObservedAt <= _availableTtl
            ? AssistantProbeResult.Available
            : AssistantProbeResult.Unknown;
    }
}
