using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class TransferSpeedTrackerTests
{
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
        var tracker = new TransferSpeedTracker();
        tracker.AddSample(0);
        Thread.Sleep(200);
        tracker.AddSample(2048);
        Thread.Sleep(200);
        tracker.AddSample(4096);

        var speed = tracker.SpeedBytesPerSecond;
        Assert.InRange(speed, 9000, 12000); // 约 10KB/s，允许计时误差
    }

    [Fact]
    public void Eta_CalculatesRemaining()
    {
        var tracker = new TransferSpeedTracker();
        tracker.AddSample(0);
        Thread.Sleep(100);
        tracker.AddSample(1000);
        Thread.Sleep(100);
        tracker.AddSample(2000); // 10KB/s

        var eta = tracker.Eta(4000);
        Assert.NotNull(eta);
        Assert.InRange(eta!.Value.TotalSeconds, 0.1, 0.5);
    }

    [Fact]
    public void Eta_Complete_ReturnsNull()
    {
        var tracker = new TransferSpeedTracker();
        tracker.AddSample(100);
        Thread.Sleep(100);
        tracker.AddSample(100);
        Assert.Null(tracker.Eta(100));
    }

    [Fact]
    public void Reset_ClearsSamples()
    {
        var tracker = new TransferSpeedTracker();
        tracker.AddSample(0);
        Thread.Sleep(100);
        tracker.AddSample(1000);
        tracker.Reset();
        Assert.Equal(0, tracker.SpeedBytesPerSecond);
    }
}
