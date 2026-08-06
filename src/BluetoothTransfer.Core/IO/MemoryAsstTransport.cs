using System.Threading.Channels;

namespace BluetoothTransfer.Core.IO;

public interface IAsstListener : IAsyncDisposable
{
    /// <summary>等待并接受一个连接；返回对端传输。</summary>
    Task<IAsstTransport> AcceptAsync(CancellationToken ct = default);
}

/// <summary>
/// 内存假传输：成对使用，写入一端的数据从另一端读出，供单元测试与自检（无需蓝牙硬件）。
/// Broken 置 true 后读端返回 0，用于模拟对端断开。
/// </summary>
public sealed class MemoryAsstTransport : IAsstTransport
{
    private readonly Channel<byte[]> _in = Channel.CreateUnbounded<byte[]>();
    private MemoryAsstTransport? _peer;
    private byte[]? _current;
    private int _pos;

    /// <summary>置 true 后本端 Read 返回 0（模拟连接断开）。</summary>
    public bool Broken { get; set; }

    private MemoryAsstTransport()
    {
    }

    /// <summary>创建一对互相连通的内存传输。</summary>
    public static (MemoryAsstTransport A, MemoryAsstTransport B) CreatePair()
    {
        var a = new MemoryAsstTransport();
        var b = new MemoryAsstTransport();
        a._peer = b;
        b._peer = a;
        return (a, b);
    }

    public async Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
    {
        if (_peer == null) throw new InvalidOperationException("传输未配对");
        await _peer._in.Writer.WriteAsync(buffer.ToArray(), ct);
    }

    public async Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
    {
        if (Broken) return 0;
        if (_current == null || _pos >= _current.Length)
        {
            _current = null;
            if (!await _in.Reader.WaitToReadAsync(ct)) return 0;
            if (!_in.Reader.TryRead(out var item)) return 0;
            _current = item;
            _pos = 0;
        }
        var n = Math.Min(buffer.Length, _current.Length - _pos);
        _current.AsSpan(_pos, n).CopyTo(buffer.Span);
        _pos += n;
        return n;
    }

    public ValueTask DisposeAsync()
    {
        _peer?._in.Writer.TryComplete();
        _in.Writer.TryComplete();
        return ValueTask.CompletedTask;
    }
}

/// <summary>
/// 内存监听器：测试/自检时把预构造的传输"连接"交给 AcceptAsync。
/// </summary>
public sealed class MemoryAsstListener : IAsstListener
{
    private readonly Channel<IAsstTransport> _connections = Channel.CreateUnbounded<IAsstTransport>();

    public void Enqueue(IAsstTransport transport)
        => _connections.Writer.TryWrite(transport);

    public async Task<IAsstTransport> AcceptAsync(CancellationToken ct = default)
    {
        if (!await _connections.Reader.WaitToReadAsync(ct))
            throw new OperationCanceledException(ct);
        return await _connections.Reader.ReadAsync(ct);
    }

    public void Close() => _connections.Writer.TryComplete();

    public ValueTask DisposeAsync()
    {
        Close();
        return ValueTask.CompletedTask;
    }
}
