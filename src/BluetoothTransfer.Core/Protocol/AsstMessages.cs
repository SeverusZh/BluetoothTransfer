using System.Buffers.Binary;
using System.Text;

namespace BluetoothTransfer.Core.Protocol;

/// <summary>助手协议消息类型（Task 1 先定义枚举，Task 2 补充载荷编解码）。</summary>
public enum AsstMessageType : byte
{
    Hello = 1,
    Offer = 2,
    Data = 3,
    Ack = 4,
    Done = 5,
    Cancel = 6,
    Error = 7
}

public enum AsstOfferStatus : byte
{
    Accept = 0,
    Busy = 1,
    Reject = 2
}

public static class AsstErrorCode
{
    public const int Busy = 1;
    public const int DiskFull = 2;
    public const int Mismatch = 3;
    public const int HashMismatch = 4;
    public const int IoError = 5;
}

public sealed record AsstHello(string TransferId, string FileName, long FileSize, int ChunkSize, string ExpectedSha256);
public sealed record AsstOffer(AsstOfferStatus Status, long ResumeOffset, string Reason = "");
public sealed record AsstData(long Offset, byte[] Payload);
public sealed record AsstAck(long Offset);
public sealed record AsstDone(bool Ok, string Hash = "");
public sealed record AsstCancel(string Reason = "");
public sealed record AsstError(int Code, string Message = "");

/// <summary>
/// 消息载荷编解码：所有整数小端序，字符串为 UTF-8 + 4 字节长度前缀。
/// </summary>
public static class AsstMessages
{
    private static void WriteString(Stream s, string value)
    {
        var bytes = Encoding.UTF8.GetBytes(value ?? "");
        Span<byte> len = stackalloc byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(len, bytes.Length);
        s.Write(len);
        s.Write(bytes);
    }

    private static string ReadString(ReadOnlySpan<byte> data, ref int pos)
    {
        if (pos + 4 > data.Length) throw new AsstProtocolException("载荷截断");
        var len = BinaryPrimitives.ReadInt32LittleEndian(data[pos..]);
        pos += 4;
        if (len < 0 || pos + len > data.Length) throw new AsstProtocolException("字符串长度非法");
        var result = Encoding.UTF8.GetString(data.Slice(pos, len));
        pos += len;
        return result;
    }

    public static byte[] EncodeHello(AsstHello h)
    {
        using var ms = new MemoryStream();
        WriteString(ms, h.TransferId);
        WriteString(ms, h.FileName);
        Span<byte> tmp = stackalloc byte[8];
        BinaryPrimitives.WriteInt64LittleEndian(tmp, h.FileSize);
        ms.Write(tmp);
        BinaryPrimitives.WriteInt32LittleEndian(tmp[..4], h.ChunkSize);
        ms.Write(tmp[..4]);
        WriteString(ms, h.ExpectedSha256);
        return ms.ToArray();
    }

    public static AsstHello DecodeHello(byte[] payload)
    {
        var pos = 0;
        var transferId = ReadString(payload, ref pos);
        var fileName = ReadString(payload, ref pos);
        if (pos + 12 > payload.Length) throw new AsstProtocolException("HELLO 载荷截断");
        var fileSize = BinaryPrimitives.ReadInt64LittleEndian(payload.AsSpan(pos));
        pos += 8;
        var chunkSize = BinaryPrimitives.ReadInt32LittleEndian(payload.AsSpan(pos));
        pos += 4;
        var sha = ReadString(payload, ref pos);
        return new AsstHello(transferId, fileName, fileSize, chunkSize, sha);
    }

    public static byte[] EncodeOffer(AsstOffer o)
    {
        using var ms = new MemoryStream();
        ms.WriteByte((byte)o.Status);
        Span<byte> tmp = stackalloc byte[8];
        BinaryPrimitives.WriteInt64LittleEndian(tmp, o.ResumeOffset);
        ms.Write(tmp);
        WriteString(ms, o.Reason);
        return ms.ToArray();
    }

    public static AsstOffer DecodeOffer(byte[] payload)
    {
        if (payload.Length < 9) throw new AsstProtocolException("OFFER 载荷截断");
        var status = (AsstOfferStatus)payload[0];
        var offset = BinaryPrimitives.ReadInt64LittleEndian(payload.AsSpan(1));
        var pos = 9;
        var reason = ReadString(payload, ref pos);
        return new AsstOffer(status, offset, reason);
    }

    public static byte[] EncodeData(AsstData d)
    {
        var payload = d.Payload ?? Array.Empty<byte>();
        var result = new byte[12 + payload.Length];
        BinaryPrimitives.WriteInt64LittleEndian(result, d.Offset);
        BinaryPrimitives.WriteInt32LittleEndian(result.AsSpan(8), payload.Length);
        payload.CopyTo(result, 12);
        return result;
    }

    public static AsstData DecodeData(byte[] payload)
    {
        if (payload.Length < 12) throw new AsstProtocolException("DATA 载荷截断");
        var offset = BinaryPrimitives.ReadInt64LittleEndian(payload);
        var len = BinaryPrimitives.ReadInt32LittleEndian(payload.AsSpan(8));
        if (len < 0 || 12 + len > payload.Length) throw new AsstProtocolException("DATA 长度非法");
        return new AsstData(offset, payload.AsSpan(12, len).ToArray());
    }

    public static byte[] EncodeAck(AsstAck a)
    {
        var result = new byte[8];
        BinaryPrimitives.WriteInt64LittleEndian(result, a.Offset);
        return result;
    }

    public static AsstAck DecodeAck(byte[] payload)
    {
        if (payload.Length < 8) throw new AsstProtocolException("ACK 载荷截断");
        return new AsstAck(BinaryPrimitives.ReadInt64LittleEndian(payload));
    }

    public static byte[] EncodeDone(AsstDone d)
    {
        using var ms = new MemoryStream();
        ms.WriteByte(d.Ok ? (byte)1 : (byte)0);
        WriteString(ms, d.Hash);
        return ms.ToArray();
    }

    public static AsstDone DecodeDone(byte[] payload)
    {
        if (payload.Length < 1) throw new AsstProtocolException("DONE 载荷截断");
        var ok = payload[0] == 1;
        var pos = 1;
        var hash = ReadString(payload, ref pos);
        return new AsstDone(ok, hash);
    }

    public static byte[] EncodeCancel(AsstCancel c)
    {
        using var ms = new MemoryStream();
        WriteString(ms, c.Reason);
        return ms.ToArray();
    }

    public static AsstCancel DecodeCancel(byte[] payload)
    {
        var pos = 0;
        return new AsstCancel(ReadString(payload, ref pos));
    }

    public static byte[] EncodeError(AsstError e)
    {
        using var ms = new MemoryStream();
        Span<byte> tmp = stackalloc byte[4];
        BinaryPrimitives.WriteInt32LittleEndian(tmp, e.Code);
        ms.Write(tmp);
        WriteString(ms, e.Message);
        return ms.ToArray();
    }

    public static AsstError DecodeError(byte[] payload)
    {
        if (payload.Length < 4) throw new AsstProtocolException("ERROR 载荷截断");
        var code = BinaryPrimitives.ReadInt32LittleEndian(payload);
        var pos = 4;
        var msg = ReadString(payload, ref pos);
        return new AsstError(code, msg);
    }
}
