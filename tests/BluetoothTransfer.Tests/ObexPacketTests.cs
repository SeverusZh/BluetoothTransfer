using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class ObexPacketTests
{
    [Fact]
    public void NameHeader_RoundTrip_ChineseAndEmoji()
    {
        const string name = "中文文件 测试 😀.txt";
        var header = ObexHeader.Name(name);

        Assert.Equal(ObexHeaderId.Name, header.Id);
        Assert.Equal(name, header.AsName());
    }

    [Fact]
    public void NameHeader_AsciiWithoutBom_Decodes()
    {
        // 兼容无 BOM、无空结尾的简化名称头
        var payload = new byte[] { 0x00, 0x61, 0x00, 0x62 }; // "ab" UTF-16BE
        var header = new ObexHeader(ObexHeaderId.Name, payload);

        Assert.Equal("ab", header.AsName());
    }

    [Fact]
    public void NameHeader_TruncatesLongNames()
    {
        var longName = new string('汉', 200) + ".txt";
        var header = ObexHeader.Name(longName);

        // 值 ≤250（UTF-16 ≤248 + 结尾空字符 2），头长度字段 = 值+3 ≤253；无 BOM
        Assert.InRange(header.Value.Length, 4, 250);
        var decoded = header.AsName();
        Assert.NotNull(decoded);
        Assert.StartsWith(new string('汉', 123), decoded);
    }

    [Fact]
    public void PutPacket_RoundTrip_AllHeaderTypes()
    {
        var packet = new ObexPacket { Opcode = (byte)ObexRequestOpcode.PutFinal };
        packet.Headers.Add(ObexHeader.Name("a.txt"));
        packet.Headers.Add(ObexHeader.Type("application/octet-stream"));
        packet.Headers.Add(ObexHeader.LengthHeader(1024));
        packet.Headers.Add(ObexHeader.Body(new byte[] { 1, 2, 3 }, true));

        var parsed = ObexPacket.Deserialize(packet.ToBytes(), out var error);

        Assert.Null(error);
        Assert.NotNull(parsed);
        Assert.Equal(packet.Opcode, parsed!.Opcode);
        Assert.Equal(4, parsed.Headers.Count);
        Assert.Equal("a.txt", parsed.Headers.First(h => h.Id == ObexHeaderId.Name).AsName());
        Assert.Equal(1024u, parsed.Headers.First(h => h.Id == ObexHeaderId.Length).AsUInt32());
        Assert.Equal(new byte[] { 1, 2, 3 }, parsed.Headers.First(h => h.Id == ObexHeaderId.EndOfBody).Value);
    }

    [Fact]
    public void ConnectRequest_HasFixedFields()
    {
        var packet = new ObexPacket
        {
            Opcode = (byte)ObexRequestOpcode.Connect,
            Version = 0x10,
            Flags = 0x00,
            MaxPacketLength = 0xFFFF
        };

        var raw = packet.ToBytes();
        Assert.Equal(7, raw.Length);
        var parsed = ObexPacket.Deserialize(raw, out _);
        Assert.NotNull(parsed);
        Assert.Equal(0x10, parsed!.Version);
        Assert.Equal(0xFFFF, parsed.MaxPacketLength);
    }

    [Fact]
    public void ConnectResponse_WithFixedFields_RoundTrip()
    {
        var packet = new ObexPacket
        {
            Opcode = ObexResponseCode.Ok,
            IncludeConnectFields = true,
            Version = 0x10,
            Flags = 0x00,
            MaxPacketLength = 4096
        };
        packet.Headers.Add(ObexHeader.ConnectionId(0x11223344));

        var parsed = ObexPacket.Deserialize(packet.ToBytes(), connectResponse: true, out var error);

        Assert.Null(error);
        Assert.NotNull(parsed);
        Assert.Equal(4096, parsed!.MaxPacketLength);
        Assert.Equal(0x11223344u, parsed.GetConnectionId());
    }

    [Theory]
    [InlineData(new byte[] { })]
    [InlineData(new byte[] { 0x20, 0x00 })]
    [InlineData(new byte[] { 0x20, 0x00, 0x02, 0x00 })]
    public void Deserialize_RejectsTruncatedPackets(byte[] data)
    {
        Assert.Null(ObexPacket.Deserialize(data, out var error));
        Assert.NotNull(error);
    }

    [Fact]
    public void Deserialize_RejectsLengthFieldMismatch()
    {
        var packet = new ObexPacket { Opcode = ObexResponseCode.Ok }.ToBytes();
        packet[1] = 0xFF; // 长度与实际不符

        Assert.Null(ObexPacket.Deserialize(packet, out var error));
        Assert.Contains("不一致", error);
    }

    [Fact]
    public void Deserialize_RejectsHeaderOverflow()
    {
        // 声明一个超出包体的头长度
        var raw = new byte[] { 0x20, 0x00, 0x08, 0x48, 0x00, 0x40, 0x01, 0x02 };
        Assert.Null(ObexPacket.Deserialize(raw, out var error));
        Assert.NotNull(error);
    }

    [Fact]
    public void AuthChallengeNonce_IsParsed()
    {
        var nonce = Enumerable.Range(0, 16).Select(i => (byte)i).ToArray();
        var challengeValue = new byte[18];
        challengeValue[0] = 0x00; // nonce 标签
        challengeValue[1] = 0x10;
        Array.Copy(nonce, 0, challengeValue, 2, 16);

        var packet = new ObexPacket { Opcode = ObexResponseCode.Unauthorized };
        packet.Headers.Add(new ObexHeader(ObexHeaderId.AuthChallenge, challengeValue));

        Assert.Equal(nonce, packet.GetAuthChallengeNonce());
    }

    [Fact]
    public void AuthDigest_IsDeterministic()
    {
        var nonce = Enumerable.Range(0, 16).Select(i => (byte)i).ToArray();
        var digest1 = ObexAuth.ComputeDigest(nonce, "pass");
        var digest2 = ObexAuth.ComputeDigest(nonce, "pass");

        Assert.Equal(16, digest1.Length);
        Assert.Equal(digest1, digest2);
    }

    [Fact]
    public void LengthHeader_RejectsOver4Gb()
    {
        Assert.Throws<ObexException>(() => ObexHeader.LengthHeader(4_294_967_296L));
    }
}
