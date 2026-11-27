using System.Buffers.Binary;
using BluetoothTransfer.Core.IO;

namespace BluetoothTransfer.Core.Protocol;

public sealed class AsstProtocolException : Exception
{
    public AsstProtocolException(string message) : base(message) { }
    public AsstProtocolException(string message, Exception inner) : base(message, inner) { }
}

/// <summary>
/// 帧格式：[4 字节长度(小端,整帧)] [1 Magic] [1 Version] [1 Type] [载荷]。
/// </summary>
public static class AsstFrame
{
    /// <summary>长度(4) + Magic(1) + Version(1) + Type(1)。</summary>
    public const int HeaderSize = 7;

    public static byte[] Build(byte type, byte[] payload)
    {
        if (payload == null) payload = Array.Empty<byte>();
        if (payload.Length > AsstConst.MaxFrameSize - HeaderSize)
            throw new AsstProtocolException($"帧载荷超出上限（{payload.Length} > {AsstConst.MaxFrameSize - HeaderSize}）");
        var total = HeaderSize + payload.Length;
        var frame = new byte[total];
        BinaryPrimitives.WriteInt32LittleEndian(frame, total);
        frame[4] = AsstConst.Magic;
        frame[5] = AsstConst.Version;
        frame[6] = type;
        payload.CopyTo(frame, HeaderSize);
        return frame;
    }

    public static (byte Type, byte[] Payload) Parse(byte[] frame)
    {
        if (frame == null || frame.Length < HeaderSize)
            throw new AsstProtocolException("帧长度不足");
        var total = BinaryPrimitives.ReadInt32LittleEndian(frame);
        if (total != frame.Length || total < HeaderSize || total > AsstConst.MaxFrameSize)
            throw new AsstProtocolException($"帧长度非法：{total}");
        if (frame[4] != AsstConst.Magic) throw new AsstProtocolException("帧魔数错误");
        if (frame[5] != AsstConst.Version) throw new AsstProtocolException("协议版本不支持");
        var payload = new byte[total - HeaderSize];
        Array.Copy(frame, HeaderSize, payload, 0, payload.Length);
        return (frame[6], payload);
    }

    public static async Task<(byte Type, byte[] Payload)> ReadFrameAsync(IAsstTransport transport, CancellationToken ct = default)
    {
        var header = new byte[HeaderSize];
        await ReadExactlyAsync(transport, header, ct);
        var total = BinaryPrimitives.ReadInt32LittleEndian(header);
        if (total < HeaderSize || total > AsstConst.MaxFrameSize)
            throw new AsstProtocolException($"帧长度非法：{total}");
        if (header[4] != AsstConst.Magic) throw new AsstProtocolException("帧魔数错误");
        if (header[5] != AsstConst.Version) throw new AsstProtocolException("协议版本不支持");
        var payload = new byte[total - HeaderSize];
        await ReadExactlyAsync(transport, payload, ct);
        return (header[6], payload);
    }

    internal static async Task ReadExactlyAsync(IAsstTransport transport, Memory<byte> buffer, CancellationToken ct)
    {
        var pos = 0;
        while (pos < buffer.Length)
        {
            var n = await transport.ReadAsync(buffer[pos..], ct);
            if (n == 0) throw new AsstProtocolException("对端提前关闭连接");
            pos += n;
        }
    }
}
