using System.IO;
using System.Security.Cryptography;
using System.Text;

namespace BluetoothTransfer.Services;

/// <summary>OBEX 请求操作码（bit7 置位表示最终包）。</summary>
public enum ObexRequestOpcode : byte
{
    Connect = 0x80,
    Disconnect = 0x81,
    Put = 0x02,
    PutFinal = 0x82,
    Get = 0x03,
    GetFinal = 0x83,
    Abort = 0x7F
}

/// <summary>OBEX 响应码常量与工具。</summary>
public static class ObexResponseCode
{
    public const byte Ok = 0x20;
    /// <summary>Success with Final bit set —— 蓝牙 OPP 规范中 CONNECT/DISCONNECT 等操作的成功响应码。</summary>
    public const byte SuccessFinal = 0xA0;
    public const byte Continue = 0x90;
    public const byte BadRequest = 0x40;
    public const byte Unauthorized = 0x41;
    public const byte Forbidden = 0x43;
    public const byte NotFound = 0x44;
    public const byte NotAcceptable = 0x46;
    public const byte RequestTimeout = 0x48;
    public const byte Conflict = 0x49;
    public const byte EntityTooLarge = 0x4D;
    public const byte UnsupportedMediaType = 0x4F;
    /// <summary>HTTP 语义 Forbidden（0xC3）——Android OPP 在已有传输进行中/被用户拒绝时返回。</summary>
    public const byte HttpForbidden = 0xC3;
    public const byte ServiceUnavailable = 0xD3;
    public const byte InternalServerError = 0xE0;

    public static bool IsSuccess(byte code) => code == Ok || code == SuccessFinal || code == Continue;

    public static string Describe(byte code) => code switch
    {
        Ok => "OK (0x20)",
        SuccessFinal => "Success Final (0xA0)",
        Continue => "Continue (0x90)",
        BadRequest => "Bad Request (0x40)",
        Unauthorized => "Unauthorized (0x41)",
        Forbidden => "Forbidden (0x43)",
        NotFound => "Not Found (0x44)",
        NotAcceptable => "Not Acceptable (0x46)",
        RequestTimeout => "Request Time Out (0x48)",
        Conflict => "Conflict (0x49)",
        EntityTooLarge => "Requested Entity Too Large (0x4D)",
        UnsupportedMediaType => "Unsupported Media Type (0x4F)",
        HttpForbidden => "Forbidden (0xC3)",
        ServiceUnavailable => "Service Unavailable (0xD3)",
        InternalServerError => "Internal Server Error (0xE0)",
        _ => $"0x{code:X2}"
    };
}

/// <summary>OBEX 头 ID 常量。</summary>
public static class ObexHeaderId
{
    public const byte Name = 0x01;
    public const byte Description = 0x05;
    public const byte Type = 0x42;
    public const byte Target = 0x46;
    public const byte Body = 0x48;
    public const byte EndOfBody = 0x49;
    public const byte Who = 0x4A;
    public const byte AuthChallenge = 0x4C;
    public const byte AuthResponse = 0x4D;
    public const byte ObjectClass = 0x4F;
    public const byte Length = 0xC3;
    public const byte ConnectionId = 0xCB;
}

/// <summary>
/// OBEX 头。编码规则由头 ID 的高两位决定：
/// 0x00-0x3F Unicode 串（2 字节长度）、0x40-0x7F 字节序列（2 字节长度）、
/// 0x80-0xBF 单字节值、0xC0-0xFF 4 字节大端值。
/// </summary>
public sealed class ObexHeader
{
    // 头长度字段上限 255：值(≤250) + 头ID(1) + 长度字段(2) + 结尾空字符(2)
    private const int MaxNameUtf16Bytes = 250;

    public byte Id { get; }
    public byte[] Value { get; }

    public ObexHeader(byte id, byte[] value)
    {
        Id = id;
        Value = value ?? throw new ArgumentNullException(nameof(value));
    }

    /// <summary>
    /// Name 头：UTF-16BE（可选 0xFEFF BOM）+ 0x0000 结尾。
    /// 与 32feet / Windows 原生发送向导一致的格式为「无 BOM」；
    /// UTF-16 字节数超过 255 时按字符边界截断（保留 2 字节结尾空字符空间）。
    /// </summary>
    public static ObexHeader Name(string name, bool useBom = false)
    {
        if (string.IsNullOrEmpty(name)) name = "file";
        // 255 字节上限含 BOM（如有）与结尾空字符，保证整头长度字段为 2 字节合法值
        var bom = useBom ? 2 : 0;
        var utf16 = EncodeUtf16(name, MaxNameUtf16Bytes - bom - 2);
        var value = new byte[bom + utf16.Length + 2];
        if (useBom)
        {
            value[0] = 0xFE;
            value[1] = 0xFF;
        }
        Array.Copy(utf16, 0, value, bom, utf16.Length);
        return new ObexHeader(ObexHeaderId.Name, value);
    }

    public static ObexHeader Type(string mime)
        => new(ObexHeaderId.Type, Encoding.UTF8.GetBytes(mime ?? "application/octet-stream"));

    public static ObexHeader LengthHeader(long size)
    {
        if (size < 0 || size > uint.MaxValue)
            throw new ObexException("文件超过 OBEX 单对象 4GB 上限");
        var v = (uint)size;
        return new ObexHeader(ObexHeaderId.Length, new[]
        {
            (byte)(v >> 24), (byte)(v >> 16), (byte)(v >> 8), (byte)v
        });
    }

    public static ObexHeader Body(byte[] data, bool end)
        => new(end ? ObexHeaderId.EndOfBody : ObexHeaderId.Body, data ?? Array.Empty<byte>());

    public static ObexHeader ConnectionId(uint id)
        => new(ObexHeaderId.ConnectionId, new[]
        {
            (byte)(id >> 24), (byte)(id >> 16), (byte)(id >> 8), (byte)id
        });

    /// <summary>认证响应：digest(16) + nonce(16)，按 OBEX 认证挑战格式编码。</summary>
    public static ObexHeader AuthResponse(byte[] digest, byte[] nonce)
    {
        if (digest == null || digest.Length != 16) throw new ArgumentException("digest 必须为 16 字节", nameof(digest));
        if (nonce == null || nonce.Length != 16) throw new ArgumentException("nonce 必须为 16 字节", nameof(nonce));
        var value = new byte[36];
        value[0] = 0x00; // digest 标签
        value[1] = 0x10;
        Array.Copy(digest, 0, value, 2, 16);
        value[18] = 0x02; // nonce 标签
        value[19] = 0x10;
        Array.Copy(nonce, 0, value, 20, 16);
        return new ObexHeader(ObexHeaderId.AuthResponse, value);
    }

    public uint? AsUInt32()
        => Value.Length == 4
            ? (uint)((Value[0] << 24) | (Value[1] << 16) | (Value[2] << 8) | Value[3])
            : null;

    /// <summary>解码 Name 头（容忍有无 BOM / 结尾空字符）。</summary>
    public string? AsName()
    {
        var payload = Value;
        if (payload.Length < 2 || payload.Length % 2 != 0) return null;
        var start = payload.Length >= 2 && payload[0] == 0xFE && payload[1] == 0xFF ? 2 : 0;
        var end = payload.Length;
        while (end - start >= 2 && payload[end - 1] == 0 && payload[end - 2] == 0) end -= 2;
        if (end - start < 2) return "";
        return Encoding.BigEndianUnicode.GetString(payload, start, end - start);
    }

    private static byte[] EncodeUtf16(string text, int maxBytes)
    {
        var bytes = Encoding.BigEndianUnicode.GetBytes(text);
        if (bytes.Length <= maxBytes) return bytes;

        var maxUnits = maxBytes / 2;
        var chars = text.ToCharArray();
        var len = Math.Min(maxUnits, chars.Length);
        // 避免截断产生孤立的高位代理项（emoji 等增补平面字符）
        if (len > 0 && chars[len - 1] >= 0xD800 && chars[len - 1] <= 0xDBFF) len--;
        var sub = new char[len];
        Array.Copy(chars, sub, len);
        return Encoding.BigEndianUnicode.GetBytes(sub);
    }
}

/// <summary>
/// OBEX 包：操作码 + 2 字节长度 + （CONNECT 固定字段）+ 头序列。
/// </summary>
public sealed class ObexPacket
{
    public byte Opcode { get; set; }
    public byte Version { get; set; } = 0x10;
    public byte Flags { get; set; }
    public ushort MaxPacketLength { get; set; } = ushort.MaxValue;
    /// <summary>序列化时是否附加 CONNECT 固定字段（version/flags/maxPacketLength），构造响应包时使用。</summary>
    public bool IncludeConnectFields { get; set; }
    public List<ObexHeader> Headers { get; } = new();

    public byte[] ToBytes()
    {
        using var content = new MemoryStream();
        if (IncludeConnectFields || Opcode == (byte)ObexRequestOpcode.Connect)
        {
            content.WriteByte(Version);
            content.WriteByte(Flags);
            content.WriteByte((byte)(MaxPacketLength >> 8));
            content.WriteByte((byte)MaxPacketLength);
        }
        foreach (var h in Headers)
            WriteHeader(content, h);

        var total = (int)content.Length + 3;
        if (total > ushort.MaxValue)
            throw new ObexException("OBEX 包长度超出 65535 上限");

        var result = new byte[total];
        result[0] = Opcode;
        result[1] = (byte)(total >> 8);
        result[2] = (byte)total;
        content.Position = 0;
        content.Read(result, 3, (int)content.Length);
        return result;
    }

    public static ObexPacket? Deserialize(byte[] data, out string? error)
        => Deserialize(data, connectResponse: false, out error);

    /// <summary>
    /// 解析 OBEX 包。<paramref name="connectResponse"/> 为 true 时按 CONNECT 响应解析
    /// （响应 0x20 后同样带有 version/flags/maxPacketLength 固定字段）。
    /// </summary>
    public static ObexPacket? Deserialize(byte[] data, bool connectResponse, out string? error)
    {
        error = null;
        if (data == null || data.Length < 3)
        {
            error = "OBEX 包长度不足 3 字节";
            return null;
        }
        var length = (data[1] << 8) | data[2];
        if (length < 3 || length != data.Length)
        {
            error = $"OBEX 长度字段({length})与实际数据({data.Length})不一致";
            return null;
        }

        var pkt = new ObexPacket { Opcode = data[0] };
        var pos = 3;
        if (connectResponse || pkt.Opcode == (byte)ObexRequestOpcode.Connect)
        {
            if (length < 7)
            {
                error = "CONNECT 包缺少固定字段";
                return null;
            }
            pkt.Version = data[pos++];
            pkt.Flags = data[pos++];
            pkt.MaxPacketLength = (ushort)((data[pos] << 8) | data[pos + 1]);
            pos += 2;
        }

        while (pos < length)
        {
            if (pos + 1 > length)
            {
                error = "头 ID 越界";
                return null;
            }
            var id = data[pos++];
            if ((id & 0xC0) == 0xC0)
            {
                if (pos + 4 > length)
                {
                    error = "4 字节头越界";
                    return null;
                }
                var v = new byte[4];
                Array.Copy(data, pos, v, 0, 4);
                pos += 4;
                pkt.Headers.Add(new ObexHeader(id, v));
            }
            else if ((id & 0xC0) == 0x80)
            {
                if (pos + 1 > length)
                {
                    error = "1 字节头越界";
                    return null;
                }
                pkt.Headers.Add(new ObexHeader(id, new[] { data[pos++] }));
            }
            else
            {
                if (pos + 2 > length)
                {
                    error = "头长度字段越界";
                    return null;
                }
                var hlen = (data[pos] << 8) | data[pos + 1];
                pos += 2;
                // 长度字段 = 头ID(1) + 长度字段(2) + 值
                if (hlen < 3 || pos + hlen - 3 > length)
                {
                    error = $"头长度({hlen})越界";
                    return null;
                }
                var v = new byte[hlen - 3];
                Array.Copy(data, pos, v, 0, hlen - 3);
                pos += hlen - 3;
                pkt.Headers.Add(new ObexHeader(id, v));
            }
        }
        return pkt;
    }

    public uint? GetConnectionId()
    {
        var header = Headers.FirstOrDefault(h => h.Id == ObexHeaderId.ConnectionId);
        return header?.AsUInt32();
    }

    /// <summary>解析认证挑战中的 Nonce（16 字节）。</summary>
    public byte[]? GetAuthChallengeNonce()
    {
        var header = Headers.FirstOrDefault(h => h.Id == ObexHeaderId.AuthChallenge);
        if (header == null) return null;
        var v = header.Value;
        var pos = 0;
        while (pos < v.Length)
        {
            var tag = v[pos++];
            switch (tag)
            {
                case 0x00: // nonce
                    if (pos + 1 + 16 > v.Length) return null;
                    pos += 1; // 跳过长度字节（固定 0x10）
                    var nonce = new byte[16];
                    Array.Copy(v, pos, nonce, 0, 16);
                    return nonce;
                case 0x01: // options（1 字节）
                    pos += 1;
                    break;
                case 0x02: // realm：[length][charset][chars]
                    if (pos + 1 > v.Length) return null;
                    pos += 1 + v[pos];
                    break;
                default:
                    return null;
            }
        }
        return null;
    }

    private static void WriteHeader(MemoryStream body, ObexHeader h)
    {
        body.WriteByte(h.Id);
        if ((h.Id & 0xC0) == 0xC0)
        {
            if (h.Value.Length != 4)
                throw new ObexException($"4 字节头 0x{h.Id:X2} 值长度必须为 4");
            body.Write(h.Value, 0, 4);
        }
        else if ((h.Id & 0xC0) == 0x80)
        {
            if (h.Value.Length != 1)
                throw new ObexException($"1 字节头 0x{h.Id:X2} 值长度必须为 1");
            body.Write(h.Value, 0, 1);
        }
        else
        {
            // OBEX 两字节长度头：长度字段 = 头 ID(1) + 长度字段本身(2) + 值。
            // Android javax.obex 按 length-3 取值长度；若写成 +2 会导致取值少 1 字节，
            // UTF-16 Name 头长度变奇数被 convertToUnicode 拒绝并断开连接。
            var len = h.Value.Length + 3;
            if (len > ushort.MaxValue)
                throw new ObexException("OBEX 头长度超出 65535 上限");
            body.WriteByte((byte)(len >> 8));
            body.WriteByte((byte)len);
            body.Write(h.Value, 0, h.Value.Length);
        }
    }
}

/// <summary>OBEX 认证工具：Digest = MD5(nonce : password)。</summary>
public static class ObexAuth
{
    public static byte[] ComputeDigest(byte[] nonce, string password)
    {
        if (nonce == null || nonce.Length != 16) throw new ArgumentException("nonce 必须为 16 字节", nameof(nonce));
        var pw = Encoding.UTF8.GetBytes(password ?? "");
        var data = new byte[nonce.Length + 1 + pw.Length];
        Array.Copy(nonce, data, nonce.Length);
        data[nonce.Length] = (byte)':';
        Array.Copy(pw, 0, data, nonce.Length + 1, pw.Length);
        return MD5.HashData(data);
    }
}

public class ObexException : Exception
{
    public ObexException(string message) : base(message) { }
    public ObexException(string message, Exception inner) : base(message, inner) { }
}
