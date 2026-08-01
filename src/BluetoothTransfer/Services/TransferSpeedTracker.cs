namespace BluetoothTransfer.Services;

/// <summary>
/// 传输速率跟踪：按时间滑动窗口统计已发送字节数，计算瞬时速率与剩余时间（ETA）。
/// 纯逻辑实现，可离线单测。
/// </summary>
public sealed class TransferSpeedTracker
{
    private readonly Queue<(DateTime Time, long Bytes)> _samples = new();
    private static readonly TimeSpan Window = TimeSpan.FromSeconds(5);
    private readonly object _lock = new();

    /// <summary>记录一个进度样本（已发送总字节数）。</summary>
    public void AddSample(long totalBytesSent)
    {
        var now = DateTime.UtcNow;
        lock (_lock)
        {
            _samples.Enqueue((now, totalBytesSent));
            while (_samples.Count > 0 && now - _samples.Peek().Time > Window)
                _samples.Dequeue();
        }
    }

    /// <summary>窗口内平均速率（字节/秒）。样本不足或窗口无进展时返回 0。</summary>
    public double SpeedBytesPerSecond
    {
        get
        {
            lock (_lock)
            {
                if (_samples.Count < 2) return 0;
                var first = _samples.Peek();
                var last = _samples.Last();
                var elapsed = (last.Time - first.Time).TotalSeconds;
                if (elapsed <= 0) return 0;
                var bytes = last.Bytes - first.Bytes;
                return bytes <= 0 ? 0 : bytes / elapsed;
            }
        }
    }

    /// <summary>基于当前速率估算剩余时间；无速率或已超过总量时返回 null。</summary>
    public TimeSpan? Eta(long totalBytes)
    {
        long current;
        lock (_lock) current = _samples.Count == 0 ? 0 : _samples.Last().Bytes;
        var speed = SpeedBytesPerSecond;
        if (speed <= 0 || totalBytes <= current) return null;
        return TimeSpan.FromSeconds((totalBytes - current) / speed);
    }

    public void Reset()
    {
        lock (_lock) _samples.Clear();
    }
}
