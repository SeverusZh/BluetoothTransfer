using System.Buffers.Binary;
using BluetoothTransfer.Core.Protocol;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AsstMessagesTests
{
    [Fact]
    public void Hello_Roundtrip_ChineseAndEmoji()
    {
        var hello = new AsstHello("transfer-1", "中文文件 测试 😀.bin", 1234567890123, 32768,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA");
        var decoded = AsstMessages.DecodeHello(AsstMessages.EncodeHello(hello));

        Assert.Equal(hello, decoded);
    }

    [Fact]
    public void Offer_Roundtrip_AllStatuses()
    {
        Assert.Equal(new AsstOffer(AsstOfferStatus.Accept, 0, ""),
            AsstMessages.DecodeOffer(AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 0))));
        Assert.Equal(new AsstOffer(AsstOfferStatus.Busy, 0, "忙"),
            AsstMessages.DecodeOffer(AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Busy, 0, "忙"))));
        Assert.Equal(new AsstOffer(AsstOfferStatus.Reject, 42, "拒绝"),
            AsstMessages.DecodeOffer(AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Reject, 42, "拒绝"))));
    }

    [Fact]
    public void Data_Roundtrip_WithBytes()
    {
        var payload = new byte[] { 0, 1, 2, 3, 255 };
        var decoded = AsstMessages.DecodeData(AsstMessages.EncodeData(new AsstData(65536, payload)));

        Assert.Equal(65536, decoded.Offset);
        Assert.Equal(payload, decoded.Payload);
    }

    [Fact]
    public void Ack_Done_Cancel_Error_Roundtrip()
    {
        Assert.Equal(new AsstAck(999), AsstMessages.DecodeAck(AsstMessages.EncodeAck(new AsstAck(999))));
        Assert.Equal(new AsstDone(true, "HASH"), AsstMessages.DecodeDone(AsstMessages.EncodeDone(new AsstDone(true, "HASH"))));
        Assert.Equal(new AsstDone(false, ""), AsstMessages.DecodeDone(AsstMessages.EncodeDone(new AsstDone(false))));
        Assert.Equal(new AsstCancel("暂停"), AsstMessages.DecodeCancel(AsstMessages.EncodeCancel(new AsstCancel("暂停"))));
        Assert.Equal(new AsstError(AsstErrorCode.HashMismatch, "校验失败"),
            AsstMessages.DecodeError(AsstMessages.EncodeError(new AsstError(AsstErrorCode.HashMismatch, "校验失败"))));
    }

    [Fact]
    public void Decode_TruncatedPayload_Throws()
    {
        Assert.Throws<AsstProtocolException>(() => AsstMessages.DecodeAck(new byte[3]));
        Assert.Throws<AsstProtocolException>(() => AsstMessages.DecodeOffer(new byte[8]));
    }

    [Fact]
    public void Decode_StringLengthInt32Max_Rejected()
    {
        // OFFER：状态(1) + 偏移(8) + reason 长度声明 Int32.MaxValue（无实际内容）
        var payload = new byte[13];
        BinaryPrimitives.WriteInt64LittleEndian(payload.AsSpan(1), 0);
        BinaryPrimitives.WriteInt32LittleEndian(payload.AsSpan(9), int.MaxValue);
        // 加法回绕曾绕过边界校验（可错误类型），现应稳定抛 AsstProtocolException
        Assert.Throws<AsstProtocolException>(() => AsstMessages.DecodeOffer(payload));
    }

    [Fact]
    public void Decode_DataLenInt32Max_Rejected()
    {
        var payload = new byte[12];
        BinaryPrimitives.WriteInt64LittleEndian(payload, 0);
        BinaryPrimitives.WriteInt32LittleEndian(payload.AsSpan(8), int.MaxValue);
        Assert.Throws<AsstProtocolException>(() => AsstMessages.DecodeData(payload));
    }
}
