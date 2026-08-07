using BluetoothTransfer.Core.Protocol;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AsstFrameTests
{
    [Fact]
    public void Build_Parse_Roundtrip()
    {
        var payload = new byte[] { 1, 2, 3, 4, 5 };
        var frame = AsstFrame.Build((byte)AsstMessageType.Hello, payload);

        Assert.Equal(AsstFrame.HeaderSize + payload.Length, frame.Length);
        var (type, data) = AsstFrame.Parse(frame);
        Assert.Equal((byte)AsstMessageType.Hello, type);
        Assert.Equal(payload, data);
    }

    [Fact]
    public void Build_OversizedPayload_Throws()
    {
        var payload = new byte[AsstConst.MaxFrameSize];
        Assert.Throws<AsstProtocolException>(() => AsstFrame.Build(1, payload));
    }

    [Fact]
    public void Parse_TamperedLength_Throws()
    {
        var frame = AsstFrame.Build(1, new byte[] { 9 });
        frame[0] = 0xFF;
        Assert.Throws<AsstProtocolException>(() => AsstFrame.Parse(frame));
    }

    [Fact]
    public void Parse_BadMagic_Throws()
    {
        var frame = AsstFrame.Build(1, Array.Empty<byte>());
        frame[4] = 0x00;
        Assert.Throws<AsstProtocolException>(() => AsstFrame.Parse(frame));
    }
}
