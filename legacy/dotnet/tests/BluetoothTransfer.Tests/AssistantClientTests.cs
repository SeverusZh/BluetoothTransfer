using BluetoothTransfer.Core.Client;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AssistantClientTests
{
    private static AsstHello Hello(long size, string sha = "", int chunk = 4096)
        => new("t1", "a.bin", size, chunk, sha);

    private static Func<long, Memory<byte>, CancellationToken, Task<int>> ReadFrom(byte[] source)
        => (offset, buffer, ct) =>
        {
            var n = Math.Min(buffer.Length, source.Length - (int)offset);
            source.AsSpan((int)offset, n).CopyTo(buffer.Span);
            return Task.FromResult(n);
        };

    [Fact]
    public async Task SendAsync_Accept_StreamsAllDataAndReturnsOk()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[100_000];
        Random.Shared.NextBytes(source);
        var received = new List<byte>();

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source), null, CancellationToken.None);

        var hello = AsstMessages.DecodeHello((await AsstFrame.ReadFrameAsync(serverT)).Payload);
        Assert.Equal("a.bin", hello.FileName);
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 0))));

        long offset = 0;
        while (offset < source.Length)
        {
            var frame = await AsstFrame.ReadFrameAsync(serverT);
            Assert.Equal((byte)AsstMessageType.Data, frame.Type);
            var data = AsstMessages.DecodeData(frame.Payload);
            Assert.Equal(offset, data.Offset);
            offset += data.Payload.Length;
            received.AddRange(data.Payload);
            await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Ack,
                AsstMessages.EncodeAck(new AsstAck(offset))));
        }
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Done,
            AsstMessages.EncodeDone(new AsstDone(true, "HASH"))));

        var result = await sendTask;

        Assert.True(result.Ok);
        Assert.Equal(source.Length, result.BytesSent);
        Assert.Equal(source, received.ToArray());
    }

    [Fact]
    public async Task SendAsync_Busy_ReturnsErrorWithoutSendingData()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[1024];

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source));

        await AsstFrame.ReadFrameAsync(serverT); // HELLO
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Busy, 0, "接收端忙"))));

        var result = await sendTask;

        Assert.False(result.Ok);
        Assert.Contains("忙", result.Error);
    }

    [Fact]
    public async Task SendAsync_Resume_FirstDataStartsAtOffset()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[20_000];
        Random.Shared.NextBytes(source);

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source));

        await AsstFrame.ReadFrameAsync(serverT); // HELLO
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 4096))));

        var frame = await AsstFrame.ReadFrameAsync(serverT);
        var data = AsstMessages.DecodeData(frame.Payload);
        Assert.Equal(4096, data.Offset);
        Assert.Equal(source.AsSpan(4096, data.Payload.Length).ToArray(), data.Payload);
    }

    [Fact]
    public async Task SendAsync_HashMismatch_ReturnsError()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[4096];

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source));

        await AsstFrame.ReadFrameAsync(serverT); // HELLO
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 0))));

        while (true)
        {
            var frame = await AsstFrame.ReadFrameAsync(serverT);
            if (frame.Type == (byte)AsstMessageType.Data)
            {
                var data = AsstMessages.DecodeData(frame.Payload);
                await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Ack,
                    AsstMessages.EncodeAck(new AsstAck(data.Offset + data.Payload.Length))));
                if (data.Offset + data.Payload.Length >= source.Length)
                    break;
            }
        }
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Done,
            AsstMessages.EncodeDone(new AsstDone(false))));

        var result = await sendTask;

        Assert.False(result.Ok);
        Assert.Contains("校验失败", result.Error);
    }

    [Theory]
    [InlineData(-1)]  // ACK 偏移小于当前偏移（回退）
    [InlineData(-100)] // ACK 偏移为负
    public async Task SendAsync_RegressingOrNegativeAckOffset_Rejected(long ackOffset)
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[20_000];

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source));

        await AsstFrame.ReadFrameAsync(serverT); // HELLO
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 0))));

        var frame = await AsstFrame.ReadFrameAsync(serverT); // DATA
        Assert.Equal((byte)AsstMessageType.Data, frame.Type);
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Ack,
            AsstMessages.EncodeAck(new AsstAck(ackOffset))));

        var ex = await Assert.ThrowsAsync<AsstProtocolException>(() => sendTask);
        Assert.Contains("非法 ACK 偏移", ex.Message);
    }

    [Fact]
    public async Task SendAsync_AckOffsetExceedsFileSize_Rejected()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[20_000];
        var ackOffset = source.Length + 1; // 超过文件大小

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source));

        await AsstFrame.ReadFrameAsync(serverT); // HELLO
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 0))));

        var frame = await AsstFrame.ReadFrameAsync(serverT); // DATA
        Assert.Equal((byte)AsstMessageType.Data, frame.Type);
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Ack,
            AsstMessages.EncodeAck(new AsstAck(ackOffset))));

        var ex = await Assert.ThrowsAsync<AsstProtocolException>(() => sendTask);
        Assert.Contains("非法 ACK 偏移", ex.Message);
    }

    [Fact]
    public async Task SendAsync_Cancelled_SendsCancelFrame()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        await using var client = new AssistantClient(clientT);
        var source = new byte[1_000_000];
        using var cts = new CancellationTokenSource();

        var sendTask = client.SendAsync(Hello(source.Length), ReadFrom(source), null, cts.Token);

        await AsstFrame.ReadFrameAsync(serverT); // HELLO
        await serverT.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
            AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, 0))));

        cts.Cancel();

        var frame = await AsstFrame.ReadFrameAsync(serverT);
        Assert.Equal((byte)AsstMessageType.Cancel, frame.Type);
        await Assert.ThrowsAsync<OperationCanceledException>(() => sendTask);
    }
}
