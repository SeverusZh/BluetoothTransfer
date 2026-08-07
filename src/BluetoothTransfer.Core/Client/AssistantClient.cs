using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;

namespace BluetoothTransfer.Core.Client;

public sealed record AsstTransferResult(bool Ok, string Error, long BytesSent, long TotalBytes, long ResumeOffset);

/// <summary>
/// 发送端协议客户端：HELLO → OFFER（含续传偏移）→ DATA/ACK 循环 → DONE。
/// 取消时先发 CANCEL 再抛 OperationCanceledException，让接收端保留半成品。
/// </summary>
public sealed class AssistantClient
{
    private readonly IAsstTransport _transport;

    public AssistantClient(IAsstTransport transport)
    {
        _transport = transport ?? throw new ArgumentNullException(nameof(transport));
    }

    public async Task<AsstTransferResult> SendAsync(
        AsstHello hello,
        Func<long, Memory<byte>, CancellationToken, Task<int>> readChunk,
        Action<long, long>? onProgress = null,
        CancellationToken ct = default)
    {
        if (hello.ChunkSize <= 0 || hello.ChunkSize > AsstConst.MaxFrameSize - AsstFrame.HeaderSize)
            throw new AsstProtocolException($"非法分片大小：{hello.ChunkSize}");

        await _transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Hello, AsstMessages.EncodeHello(hello)), ct);

        var (offerType, offerPayload) = await AsstFrame.ReadFrameAsync(_transport, ct);
        if (offerType != (byte)AsstMessageType.Offer)
            throw new AsstProtocolException($"期望 OFFER，收到消息类型 {offerType}");
        var offer = AsstMessages.DecodeOffer(offerPayload);
        if (offer.Status == AsstOfferStatus.Busy)
            return new AsstTransferResult(false, "接收端忙，稍后自动重试", 0, hello.FileSize, 0);
        if (offer.Status == AsstOfferStatus.Reject)
            return new AsstTransferResult(false, string.IsNullOrEmpty(offer.Reason) ? "接收端拒绝" : offer.Reason, 0, hello.FileSize, 0);
        if (offer.ResumeOffset < 0 || offer.ResumeOffset > hello.FileSize)
            throw new AsstProtocolException($"非法续传偏移：{offer.ResumeOffset}");

        var current = offer.ResumeOffset;
        var resumeOffset = offer.ResumeOffset;
        onProgress?.Invoke(current, hello.FileSize);

        try
        {
            if (current < hello.FileSize)
            {
                while (true)
                {
                    if (ct.IsCancellationRequested)
                    {
                        await TrySendCancelAsync();
                        ct.ThrowIfCancellationRequested();
                    }
                    var chunk = new byte[hello.ChunkSize];
                    var n = await readChunk(current, chunk, ct);
                    if (n <= 0)
                        throw new AsstProtocolException("源文件读取提前结束");
                    await _transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Data,
                        AsstMessages.EncodeData(new AsstData(current, chunk.AsSpan(0, n).ToArray()))), ct);
                    var (ackType, ackPayload) = await AsstFrame.ReadFrameAsync(_transport, ct);
                    if (ackType != (byte)AsstMessageType.Ack)
                        throw new AsstProtocolException($"期望 ACK，收到消息类型 {ackType}");
                    current = AsstMessages.DecodeAck(ackPayload).Offset;
                    onProgress?.Invoke(current, hello.FileSize);
                    if (current >= hello.FileSize) break;
                }
            }
        }
        catch (OperationCanceledException)
        {
            await TrySendCancelAsync();
            throw;
        }

        var (doneType, donePayload) = await AsstFrame.ReadFrameAsync(_transport, ct);
        if (doneType != (byte)AsstMessageType.Done)
            throw new AsstProtocolException($"期望 DONE，收到消息类型 {doneType}");
        var done = AsstMessages.DecodeDone(donePayload);
        if (!done.Ok)
            return new AsstTransferResult(false, "接收端 SHA-256 校验失败（已清空半成品，重试将从头传输）", current, hello.FileSize, resumeOffset);
        return new AsstTransferResult(true, "", current, hello.FileSize, resumeOffset);
    }

    private async Task TrySendCancelAsync()
    {
        try
        {
            await _transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Cancel,
                AsstMessages.EncodeCancel(new AsstCancel("用户暂停/取消"))), CancellationToken.None);
        }
        catch
        {
            // 连接可能已断开，忽略
        }
    }

    public ValueTask DisposeAsync() => _transport.DisposeAsync();
}
