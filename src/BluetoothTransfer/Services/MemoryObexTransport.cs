namespace BluetoothTransfer.Services;

/// <summary>
/// 内存假传输：预置响应包队列，记录全部写入字节，供单元测试与 selftest 使用（无需蓝牙硬件）。
/// </summary>
public sealed class MemoryObexTransport : IObexTransport
{
    private readonly object _lock = new();
    private readonly Queue<byte[]> _responses = new();
    private readonly List<byte> _written = new();
    private byte[]? _current;
    private int _currentPos;

    public MemoryObexTransport()
    {
    }

    public MemoryObexTransport(params byte[][] responses)
    {
        foreach (var response in responses) EnqueueResponse(response);
    }

    /// <summary>
    /// 自动应答模式：CONNECT→OK、PUT→Continue、PUT 最终包→OK、DISCONNECT→OK。
    /// 开启后无需手工排队响应，适合流程类测试。
    /// </summary>
    public bool AutoReply { get; set; }

    public void EnqueueResponse(byte[] packet)
    {
        lock (_lock) _responses.Enqueue(packet);
    }

    public byte[] WrittenBytes
    {
        get
        {
            lock (_lock) return _written.ToArray();
        }
    }

    /// <summary>尝试把全部写入字节按 OBEX 包切分解析；任何包非法则返回 null。</summary>
    public IReadOnlyList<ObexPacket>? ParseWrittenPackets()
    {
        lock (_lock)
        {
            var packets = new List<ObexPacket>();
            var pos = 0;
            while (pos + 3 <= _written.Count)
            {
                var length = (_written[pos + 1] << 8) | _written[pos + 2];
                if (length < 3 || pos + length > _written.Count) return null;
                var slice = new byte[length];
                _written.CopyTo(pos, slice, 0, length);
                var pkt = ObexPacket.Deserialize(slice, out _);
                if (pkt == null) return null;
                packets.Add(pkt);
                pos += length;
            }
            return pos == _written.Count ? packets : null;
        }
    }

    public Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
    {
        lock (_lock)
        {
            var bytes = buffer.ToArray();
            _written.AddRange(bytes);
            if (AutoReply)
                _responses.Enqueue(AutoResponseFor(bytes[0]));
        }
        return Task.CompletedTask;
    }

    public Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
    {
        lock (_lock)
        {
            if (_current == null || _currentPos >= _current.Length)
            {
                if (_responses.Count == 0) return Task.FromResult(0);
                _current = _responses.Dequeue();
                _currentPos = 0;
            }
            var n = Math.Min(buffer.Length, _current.Length - _currentPos);
            _current.AsSpan(_currentPos, n).CopyTo(buffer.Span);
            _currentPos += n;
            return Task.FromResult(n);
        }
    }

    public ValueTask DisposeAsync()
    {
        lock (_lock)
        {
            _current = null;
            _responses.Clear();
        }
        return ValueTask.CompletedTask;
    }

    private static byte[] AutoResponseFor(byte opcode)
    {
        if (opcode == (byte)ObexRequestOpcode.Connect)
        {
            return new ObexPacket
            {
                Opcode = ObexResponseCode.Ok,
                IncludeConnectFields = true,
                Version = 0x10,
                Flags = 0x00,
                MaxPacketLength = ushort.MaxValue
            }.ToBytes();
        }
        if (opcode == (byte)ObexRequestOpcode.Put)
            return new ObexPacket { Opcode = ObexResponseCode.Continue }.ToBytes();
        return new ObexPacket { Opcode = ObexResponseCode.Ok }.ToBytes();
    }
}
