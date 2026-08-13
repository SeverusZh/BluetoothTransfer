using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;

namespace BluetoothTransfer.Core.Server;

/// <summary>
/// 接收端协议服务：监听自定义 RFCOMM 服务，单活跃传输；
/// 忙时对新连接回 OFFER(Busy)；支持续传协商、SHA-256 校验、取消保留半成品。
/// </summary>
public sealed class AssistantServer
{
    private readonly IAsstListener _listener;
    private readonly IAsstSink _sink;
    private readonly Func<AsstHello, Task<AsstOfferStatus>>? _approval;
    private readonly Action<string, string>? _onLog;
    private readonly Action<AsstHello, long, long>? _onProgress;
    private readonly Action<AsstHello, string?>? _onCompleted;
    private readonly CancellationTokenSource _cts = new();
    private int _active;

    public AssistantServer(
        IAsstListener listener,
        IAsstSink sink,
        Func<AsstHello, Task<AsstOfferStatus>>? approval = null,
        Action<string, string>? onLog = null,
        Action<AsstHello, long, long>? onProgress = null,
        Action<AsstHello, string?>? onCompleted = null)
    {
        _listener = listener ?? throw new ArgumentNullException(nameof(listener));
        _sink = sink ?? throw new ArgumentNullException(nameof(sink));
        _approval = approval;
        _onLog = onLog;
        _onProgress = onProgress;
        _onCompleted = onCompleted;
    }

    public async Task RunAsync(CancellationToken ct = default)
    {
        using var linked = CancellationTokenSource.CreateLinkedTokenSource(ct, _cts.Token);
        while (!linked.IsCancellationRequested)
        {
            IAsstTransport transport;
            try
            {
                transport = await _listener.AcceptAsync(linked.Token);
            }
            catch (OperationCanceledException)
            {
                break;
            }
            if (Interlocked.Exchange(ref _active, 1) == 1)
            {
                _ = SendBusyAndCloseAsync(transport);
                continue;
            }
            _ = Task.Run(async () =>
            {
                try
                {
                    await HandleAsync(transport, linked.Token);
                }
                catch (Exception ex)
                {
                    _onLog?.Invoke("ERROR", $"传输处理失败：{ex.Message}");
                }
                finally
                {
                    Interlocked.Exchange(ref _active, 0);
                }
            }, CancellationToken.None);
        }
    }

    private async Task SendBusyAndCloseAsync(IAsstTransport transport)
    {
        try
        {
            await transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
                AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Busy, 0, "接收端忙"))));
        }
        catch
        {
            // 连接已断开则忽略
        }
        await transport.DisposeAsync();
    }

    /// <summary>处理单个连接（测试可直接驱动）。</summary>
    public async Task HandleAsync(IAsstTransport transport, CancellationToken ct = default)
    {
        AsstHello? hello = null;
        try
        {
            var (type, payload) = await AsstFrame.ReadFrameAsync(transport, ct);
            if (type != (byte)AsstMessageType.Hello)
                throw new AsstProtocolException("首个消息必须是 HELLO");
            hello = AsstMessages.DecodeHello(payload);
            _onLog?.Invoke("INFO", $"收到传输请求：{hello.FileName}（{hello.FileSize} 字节）");

            var status = _approval == null ? AsstOfferStatus.Accept : await _approval(hello);
            if (status == AsstOfferStatus.Reject)
            {
                await transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
                    AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Reject, 0, "用户拒绝"))));
                return;
            }

            var offset = await _sink.OpenAsync(hello, ct);
            await transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Offer,
                AsstMessages.EncodeOffer(new AsstOffer(AsstOfferStatus.Accept, offset))));
            _onLog?.Invoke("INFO", $"已接受，续传偏移 {offset}");

            var current = offset;
            if (current >= hello.FileSize)
            {
                await FinishAsync(transport, hello, ct);
                return;
            }

            while (true)
            {
                var (t2, p2) = await AsstFrame.ReadFrameAsync(transport, ct);
                switch ((AsstMessageType)t2)
                {
                    case AsstMessageType.Data:
                        var data = AsstMessages.DecodeData(p2);
                        if (data.Offset != current)
                            throw new AsstProtocolException($"数据偏移不一致：期望 {current}，收到 {data.Offset}");
                        await _sink.WriteAsync(hello, data.Offset, data.Payload, ct);
                        current += data.Payload.Length;
                        await transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Ack,
                            AsstMessages.EncodeAck(new AsstAck(current))), ct);
                        _onProgress?.Invoke(hello, current, hello.FileSize);
                        if (current >= hello.FileSize)
                        {
                            await FinishAsync(transport, hello, ct);
                            return;
                        }
                        break;
                    case AsstMessageType.Cancel:
                        await _sink.AbortAsync(hello, deletePartial: false, ct);
                        _onLog?.Invoke("INFO", $"传输已取消（保留半成品）：{hello.FileName}");
                        return;
                    case AsstMessageType.Error:
                        await _sink.AbortAsync(hello, deletePartial: false, ct);
                        _onLog?.Invoke("INFO", $"对端报错，已保留半成品：{hello.FileName}");
                        return;
                    default:
                        throw new AsstProtocolException($"协议错误：意外消息 {t2}");
                }
            }
        }
        // 断点续传缺陷修复：蓝牙传输最常见的失败形态是连接中断（对端提前关闭/IO 异常，见
        // AsstProtocolException"对端提前关闭连接"）。此前协议异常与一般异常分支会 deletePartial:
        // true 删掉 .btpart/.btpart.meta，而发送端对断连会重试，重连后重发 HELLO，接收端因半成品
        // 已删只能返回偏移 0，导致从头重传，断点续传名存实亡。
        // 因此 HELLO 之后的任何传输中途失败（含协议异常与一般异常）一律保留半成品（deletePartial:
        // false）。保留在协议上安全，理由有三：
        //   1) 检查点可靠：WriteAsync 在发 ACK 前已 Flush(flushToDisk:true) 落盘，偏移即已确认进度；
        //   2) meta 防脏续传：OpenAsync 按 meta（TransferId + FileSize）校验续传合法性，脏残留不可复用；
        //   3) SHA-256 最终把关：CompleteAsync 校验哈希，不匹配时清空半成品，坏数据不会变成正式文件。
        catch (AsstProtocolException ex)
        {
            _onLog?.Invoke("ERROR", ex.Message);
            await TrySendErrorAsync(transport, ex.Message);
            if (hello != null) await _sink.AbortAsync(hello, deletePartial: false, ct);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            // 宿主取消：保留半成品（可续传）
            if (hello != null) await _sink.AbortAsync(hello, deletePartial: false, ct);
        }
        catch (Exception ex)
        {
            _onLog?.Invoke("ERROR", $"接收失败：{ex.Message}");
            await TrySendErrorAsync(transport, ex.Message);
            if (hello != null) await _sink.AbortAsync(hello, deletePartial: false, ct);
        }
        finally
        {
            await transport.DisposeAsync();
        }
    }

    private async Task FinishAsync(IAsstTransport transport, AsstHello hello, CancellationToken ct)
    {
        var done = await _sink.CompleteAsync(hello, ct);
        await transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Done,
            AsstMessages.EncodeDone(done)), ct);
        _onLog?.Invoke(done.Ok ? "INFO" : "ERROR",
            done.Ok
                ? $"接收完成：{hello.FileName}（SHA-256 {done.Hash[..Math.Min(8, done.Hash.Length)]}…）"
                : $"校验失败：{hello.FileName}（期望 {hello.ExpectedSha256}，实际 {done.Hash}）");
        if (done.Ok) _onCompleted?.Invoke(hello, transport.RemoteAddress);
    }

    private async Task TrySendErrorAsync(IAsstTransport transport, string message)
    {
        try
        {
            await transport.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Error,
                AsstMessages.EncodeError(new AsstError(AsstErrorCode.IoError, message))), CancellationToken.None);
        }
        catch
        {
            // 对端可能已断开
        }
    }

    public ValueTask DisposeAsync()
    {
        _cts.Cancel();
        return ValueTask.CompletedTask;
    }
}
