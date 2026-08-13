using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class TransferSpeedTrackerTests
{
    private static TransferSpeedTracker CreateTracker(ManualTimeProvider clock) => new(clock);

    [Fact]
    public void Speed_NoSamples_Zero()
    {
        var tracker = new TransferSpeedTracker();
        Assert.Equal(0, tracker.SpeedBytesPerSecond);
        Assert.Null(tracker.Eta(100));
    }

    [Fact]
    public void Speed_SingleSample_Zero()
    {
        var tracker = new TransferSpeedTracker();
        tracker.AddSample(100);
        Assert.Equal(0, tracker.SpeedBytesPerSecond);
    }

    [Fact]
    public void Speed_StableRate_WithinWindow()
    {
        var clock = new ManualTimeProvider();
        var tracker = CreateTracker(clock);

        // 每 200ms 递增 2048 字节，速率恒为 10KB/s。
        tracker.AddSample(0);
        clock.Advance(TimeSpan.FromMilliseconds(200));
        tracker.AddSample(2048);
        clock.Advance(TimeSpan.FromMilliseconds(200));
        tracker.AddSample(4096);

        Assert.Equal(10240, tracker.SpeedBytesPerSecond);
    }

    [Fact]
    public void Eta_CalculatesRemaining()
    {
        var clock = new ManualTimeProvider();
        var tracker = CreateTracker(clock);

        // 每 100ms 递增 1000 字节，速率恒为 10KB/s。
        tracker.AddSample(0);
        clock.Advance(TimeSpan.FromMilliseconds(100));
        tracker.AddSample(1000);
        clock.Advance(TimeSpan.FromMilliseconds(100));
        tracker.AddSample(2000); // 10KB/s

        var eta = tracker.Eta(4000);
        Assert.NotNull(eta);
        Assert.Equal(0.2, eta!.Value.TotalSeconds); // (4000-2000)/10000 = 0.2s
    }

    [Fact]
    public void Eta_Complete_ReturnsNull()
    {
        var clock = new ManualTimeProvider();
        var tracker = CreateTracker(clock);

        tracker.AddSample(100);
        clock.Advance(TimeSpan.FromMilliseconds(100));
        tracker.AddSample(100);
        Assert.Null(tracker.Eta(100));
    }

    [Fact]
    public void Reset_ClearsSamples()
    {
        var clock = new ManualTimeProvider();
        var tracker = CreateTracker(clock);

        tracker.AddSample(0);
        clock.Advance(TimeSpan.FromMilliseconds(100));
        tracker.AddSample(1000);
        tracker.Reset();
        Assert.Equal(0, tracker.SpeedBytesPerSecond);
    }

    /// <summary>
    /// 受控时钟：测试中以固定步长推进时间，消除真实耗时带来的不确定性。
    /// </summary>
    private sealed class ManualTimeProvider : TimeProvider
    {
        private DateTimeOffset _now = DateTimeOffset.UtcNow;

        public override DateTimeOffset GetUtcNow() => _now;

        public void Advance(TimeSpan delta) => _now += delta;
    }
}
