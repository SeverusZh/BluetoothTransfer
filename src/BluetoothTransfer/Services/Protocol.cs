using System.Text;

namespace BluetoothTransfer.Services;

public enum MsgType : byte
{
    META = 0x01,
    DATA = 0x02,
    ACK = 0x03,
    NACK = 0x04,
    END = 0x05,
    HEARTBEAT = 0x06,
    OPEN_RFCOMM = 0x07,
    RFCOMM_READY = 0x08,
    KEY_EXCHANGE = 0x09
}

[Flags]
public enum FrameFlags : byte
{
    None = 0,
    Encrypted = 1,
    Compressed = 2,
    FinalChunk = 4
}

public class Frame
{
    public byte Version { get; set; } = 1;
    public FrameFlags Flags { get; set; } = FrameFlags.None;
    public MsgType MsgType { get; set; }
    public uint TaskId { get; set; }
    /// <summary>
    /// 分片序号，16 位无符号滚动计数（mod 65536），超长传输（RFCOMM 4KB 分片约 256MB）后会回绕到 0。
    /// 仅用于诊断/调试；分片的有序写入与断点续传一律以 <see cref="Offset"/> 为准，接收方不得依赖 SeqNo 的单调性。
    /// </summary>
    public ushort SeqNo { get; set; }
    public uint TotalLen { get; set; }
    public uint Offset { get; set; }
    public byte[] Payload { get; set; } = Array.Empty<byte>();

    public byte[] Serialize()
    {
        var chunkLen = (ushort)Payload.Length;
        var buf = new byte[1 + 1 + 1 + 4 + 2 + 4 + 4 + 2 + chunkLen + 2];
        var pos = 0;

        buf[pos++] = Version;
        buf[pos++] = (byte)Flags;
        buf[pos++] = (byte)MsgType;
        WriteUInt32(buf, ref pos, TaskId);
        WriteUInt16(buf, ref pos, SeqNo);
        WriteUInt32(buf, ref pos, TotalLen);
        WriteUInt32(buf, ref pos, Offset);
        WriteUInt16(buf, ref pos, chunkLen);
        Array.Copy(Payload, 0, buf, pos, chunkLen);
        pos += chunkLen;

        var crc = Crc16.Compute(buf, 0, pos);
        WriteUInt16(buf, ref pos, crc);
        return buf;
    }

    public static Frame? Deserialize(byte[] data)
    {
        if (data.Length < 21) return null;

        var pos = 0;
        var frame = new Frame
        {
            Version = data[pos++],
            Flags = (FrameFlags)data[pos++],
            MsgType = (MsgType)data[pos++],
            TaskId = ReadUInt32(data, ref pos),
            SeqNo = ReadUInt16(data, ref pos),
            TotalLen = ReadUInt32(data, ref pos),
            Offset = ReadUInt32(data, ref pos)
        };
        if (frame.Version != 1)
        {
            // 协议版本不匹配：拒绝解析，避免后续字段错位导致误判。
            return null;
        }

        var chunkLen = ReadUInt16(data, ref pos);
        if (data.Length < 19 + chunkLen + 2) return null;

        frame.Payload = new byte[chunkLen];
        Array.Copy(data, pos, frame.Payload, 0, chunkLen);
        pos += chunkLen;

        var expectedCrc = ReadUInt16(data, ref pos);
        var actualCrc = Crc16.Compute(data, 0, pos - 2);
        if (expectedCrc != actualCrc) return null;

        return frame;
    }

    private static void WriteUInt32(byte[] buf, ref int pos, uint val)
    {
        buf[pos++] = (byte)(val >> 24);
        buf[pos++] = (byte)(val >> 16);
        buf[pos++] = (byte)(val >> 8);
        buf[pos++] = (byte)val;
    }

    private static void WriteUInt16(byte[] buf, ref int pos, ushort val)
    {
        buf[pos++] = (byte)(val >> 8);
        buf[pos++] = (byte)val;
    }

    private static uint ReadUInt32(byte[] buf, ref int pos)
    {
        uint val = (uint)(buf[pos] << 24 | buf[pos + 1] << 16 | buf[pos + 2] << 8 | buf[pos + 3]);
        pos += 4;
        return val;
    }

    private static ushort ReadUInt16(byte[] buf, ref int pos)
    {
        ushort val = (ushort)(buf[pos] << 8 | buf[pos + 1]);
        pos += 2;
        return val;
    }
}

/// <summary>
/// 帧校验所用 CRC-16 变体固定为 <b>CRC-16/CCITT-FALSE</b>：
/// 多项式 0x1021、初值 0xFFFF、输入/输出均不反转（refin/refout=false）、异或输出 0x0000。
/// 序列化与反序列化使用同一算法，内部自洽；与异构对端互通时须固定为该变体，否则跨端校验失败。
/// </summary>
public static class Crc16
{
    private static readonly ushort[] Table = GenerateTable();

    private static ushort[] GenerateTable()
    {
        var table = new ushort[256];
        for (int i = 0; i < 256; i++)
        {
            ushort crc = (ushort)(i << 8);
            for (int j = 0; j < 8; j++)
                crc = (crc & 0x8000) != 0 ? (ushort)((crc << 1) ^ 0x1021) : (ushort)(crc << 1);
            table[i] = crc;
        }
        return table;
    }

    public static ushort Compute(byte[] data, int offset, int length)
    {
        ushort crc = 0xFFFF;
        for (int i = offset; i < offset + length; i++)
            crc = (ushort)((crc << 8) ^ Table[((crc >> 8) ^ data[i]) & 0xFF]);
        return crc;
    }
}

public static class MetaPayload
{
    public static byte[] Encode(string type, string name, long size, string checksum, FrameFlags flags)
    {
        var json = System.Text.Json.JsonSerializer.Serialize(new
        {
            type,
            name,
            size,
            checksum,
            flags = (byte)flags
        });
        return Encoding.UTF8.GetBytes(json);
    }

    public static (string type, string name, long size, string checksum)? Decode(byte[] payload)
    {
        try
        {
            var json = Encoding.UTF8.GetString(payload);
            var doc = System.Text.Json.JsonDocument.Parse(json);
            var root = doc.RootElement;
            return (
                root.GetProperty("type").GetString() ?? "",
                root.GetProperty("name").GetString() ?? "",
                root.GetProperty("size").GetInt64(),
                root.GetProperty("checksum").GetString() ?? ""
            );
        }
        catch { return null; }
    }
}
