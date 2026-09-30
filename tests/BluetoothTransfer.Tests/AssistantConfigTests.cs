using BluetoothTransfer.Core.Discovery;
using BluetoothTransfer.Models;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AssistantConfigTests
{
    [Fact]
    public void TransferMode_DefaultsToAuto()
    {
        Assert.Equal("auto", new AppConfig().TransferMode);
    }

    [Fact]
    public void ChannelDisplay_MapsAssistant()
    {
        Assert.Equal("助手", new TransferRecord { Channel = TransferConst.ChannelAssistant }.ChannelDisplay);
        Assert.Equal("OPP", new TransferRecord { Channel = TransferConst.ChannelOpp }.ChannelDisplay);
    }
}

/// <summary>
/// 助手探测状态机（<see cref="AssistantPresenceTracker"/>）的回归测试。
/// 该纯类型不依赖 Windows/WPF/蓝牙硬件，可直接在 Linux（verifier 的 net8.0 harness）执行——
/// 是本 Bug "失败与确认无助手不可区分 + 负结果被永久缓存 + 失败把 true 降级"的可执行证据。
/// 时间源与 TTL 均为注入/可配，测试无需等待真实时间。
/// </summary>
public class AssistantProbePolicyTests
{
    private const string Addr = "aa:bb:cc:dd:ee:ff";

    private static AssistantPresenceTracker NewTracker(DateTimeOffset now, TimeSpan? ttl = null)
        => new(ttl ?? TimeSpan.FromMinutes(2), () => now);

    [Fact]
    public void Current_WithoutObservation_IsUnknown()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);
        Assert.True(tracker.Current(Addr).IsUnknown);
        Assert.True(tracker.LastObserved(Addr).IsUnknown);
    }

    [Fact]
    public void RecordAbsent_IsNotCachedAsNegativeConclusion()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);

        var effective = tracker.Record(Addr, AssistantProbeResult.Absent);

        // 当次结论是确定的"无助手"（调用方据此清徽标/走 OPP）……
        Assert.True(effective.IsAbsent);
        Assert.True(tracker.LastObserved(Addr).IsAbsent);
        // ……但不得成为长期缓存结论：Current 仍为 Unknown，下一次探测会重探。
        Assert.True(tracker.Current(Addr).IsUnknown);
    }

    [Fact]
    public void RecordFailed_WithoutPositive_StaysUnknown_ThenReprobeDiscovers()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);

        // 第一次探测遇到设备忙 → Unknown（不是"确认无助手"）。
        var failed = tracker.Record(Addr, AssistantProbeResult.Failed("设备忙"));
        Assert.True(failed.IsUnknown);
        Assert.True(tracker.Current(Addr).IsUnknown);

        // 对端助手后启动：下一次探测成功即可发现（不会被第一次失败锁死）。
        var ok = tracker.Record(Addr, AssistantProbeResult.Available);
        Assert.True(ok.IsAvailable);
        Assert.True(tracker.Current(Addr).IsAvailable);
    }

    [Fact]
    public void RecordFailed_AfterAvailable_DoesNotDowngrade()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);
        tracker.Record(Addr, AssistantProbeResult.Available);

        var effective = tracker.Record(Addr, AssistantProbeResult.Failed("SDP 查询异常"));

        // 失败不得把已确认的 true 降级。
        Assert.True(effective.IsAvailable);
        Assert.True(tracker.Current(Addr).IsAvailable);
    }

    [Fact]
    public void RecordAvailable_ExpiresAfterTtl_ThenUnknownAndReprobeRequired()
    {
        var now = DateTimeOffset.UnixEpoch;
        var tracker = new AssistantPresenceTracker(TimeSpan.FromMinutes(2), () => now);
        tracker.Record(Addr, AssistantProbeResult.Available);
        Assert.True(tracker.Current(Addr).IsAvailable);

        now = now.AddMinutes(3); // 超过 TTL
        Assert.True(tracker.Current(Addr).IsUnknown);

        // 过期后即使探测失败也不得凭旧正结果改判 Available。
        Assert.True(tracker.Record(Addr, AssistantProbeResult.Failed("超时")).IsUnknown);
    }

    [Fact]
    public void Record_StaleObservation_DoesNotOverrideNewerConclusion()
    {
        var t0 = DateTimeOffset.UnixEpoch;
        var tracker = NewTracker(t0);

        // 较新的正结果先落库（例如 MergeAssistantDevicesAsync）……
        tracker.Record(Addr, AssistantProbeResult.Available, t0.AddSeconds(10));
        // ……随后 fire-and-forget 的旧失败/旧否定结果才返回，不得覆盖。
        var staleFailed = tracker.Record(Addr, AssistantProbeResult.Failed("旧查询"), t0);
        var staleAbsent = tracker.Record(Addr, AssistantProbeResult.Absent, t0.AddSeconds(1));

        Assert.True(staleFailed.IsAvailable);
        Assert.True(staleAbsent.IsAvailable);
        Assert.True(tracker.Current(Addr).IsAvailable);
    }

    [Fact]
    public void Record_ConcurrentFailures_DoNotDowngradeAvailable()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);
        tracker.Record(Addr, AssistantProbeResult.Available);

        Parallel.For(0, 200, _ => tracker.Record(Addr, AssistantProbeResult.Failed("并发瞬时失败")));

        Assert.True(tracker.Current(Addr).IsAvailable);
    }

    [Fact]
    public void Invalidate_RemovesCachedPositive_ForcingReprobe()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);
        tracker.Record(Addr, AssistantProbeResult.Available);
        Assert.True(tracker.Current(Addr).IsAvailable);

        tracker.Invalidate(Addr);

        Assert.True(tracker.Current(Addr).IsUnknown);
    }

    [Fact]
    public void Clear_RemovesAllCachedPositives()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);
        tracker.Record(Addr, AssistantProbeResult.Available);
        tracker.Record("11:22:33:44:55:66", AssistantProbeResult.Available);

        tracker.Clear();

        Assert.True(tracker.Current(Addr).IsUnknown);
        Assert.True(tracker.Current("11:22:33:44:55:66").IsUnknown);
    }

    [Fact]
    public void Record_BlankAddress_PassesThroughWithoutCaching()
    {
        var tracker = NewTracker(DateTimeOffset.UnixEpoch);
        var effective = tracker.Record("", AssistantProbeResult.Available);
        Assert.True(effective.IsAvailable);
        Assert.True(tracker.Current("").IsUnknown);
    }
}
