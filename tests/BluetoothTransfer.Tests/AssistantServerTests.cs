using BluetoothTransfer.Core.Client;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AssistantServerTests : IDisposable
{
    private readonly string _dir;

    public AssistantServerTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_server_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch { }
    }

    private static Func<long, Memory<byte>, CancellationToken, Task<int>> ReadFrom(byte[] source)
        => (offset, buffer, ct) =>
        {
            var n = Math.Min(buffer.Length, source.Length - (int)offset);
            source.AsSpan((int)offset, n).CopyTo(buffer.Span);
            return Task.FromResult(n);
        };

    [Fact]
    public async Task FullFlow_ClientToServer_SavesVerifiedFile()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        var sink = new AsstFileSink(_dir);
        var server = new AssistantServer(new MemoryAsstListener(), sink);
        var handleTask = server.HandleAsync(serverT, CancellationToken.None);

        var source = new byte[100_000];
        Random.Shared.NextBytes(source);
        var sourcePath = Path.Combine(_dir, "src.bin");
        await File.WriteAllBytesAsync(sourcePath, source);
        var sha = await AsstHash.ComputeSha256Async(sourcePath);
        var hello = new AsstHello("f1", "out.bin", source.Length, 8192, sha);

        await using var client = new AssistantClient(clientT);
        var result = await client.SendAsync(hello, ReadFrom(source));

        await handleTask;

        Assert.True(result.Ok);
        Assert.Equal(source.Length, result.BytesSent);
        Assert.Equal(source, await File.ReadAllBytesAsync(Path.Combine(_dir, "out.bin")));
    }

    [Fact]
    public async Task Resume_InterruptedSession_CompletesFromOffset()
    {
        var sink = new AsstFileSink(_dir);
        var source = new byte[200_000];
        Random.Shared.NextBytes(source);
        var sourcePath = Path.Combine(_dir, "src.bin");
        await File.WriteAllBytesAsync(sourcePath, source);
        var sha = await AsstHash.ComputeSha256Async(sourcePath);
        var hello = new AsstHello("resume-1", "big.bin", source.Length, 8192, sha);

        // 会话 1：传一半后取消，接收端保留半成品
        var (ct1, st1) = MemoryAsstTransport.CreatePair();
        var server1 = new AssistantServer(new MemoryAsstListener(), sink);
        var handle1 = server1.HandleAsync(st1, CancellationToken.None);
        await using var client1 = new AssistantClient(ct1);
        using var cts = new CancellationTokenSource();
        var progressSeen = new TaskCompletionSource<long>();
        var send1 = client1.SendAsync(hello, ReadFrom(source), (sent, _) =>
        {
            if (sent > 0) progressSeen.TrySetResult(sent);
        }, cts.Token);
        await progressSeen.Task;
        cts.Cancel();
        await Assert.ThrowsAsync<OperationCanceledException>(() => send1);
        await handle1;

        Assert.True(File.Exists(Path.Combine(_dir, "big.bin.btpart")));

        // 会话 2：重新连接，从偏移续传并完成
        var (ct2, st2) = MemoryAsstTransport.CreatePair();
        var server2 = new AssistantServer(new MemoryAsstListener(), sink);
        var handle2 = server2.HandleAsync(st2, CancellationToken.None);
        await using var client2 = new AssistantClient(ct2);
        var result = await client2.SendAsync(hello, ReadFrom(source));

        await handle2;

        Assert.True(result.Ok);
        Assert.Equal(source.Length, result.BytesSent);
        Assert.Equal(source, await File.ReadAllBytesAsync(Path.Combine(_dir, "big.bin")));
    }

    [Fact]
    public async Task Busy_SecondConnection_GetsBusyOffer()
    {
        var listener = new MemoryAsstListener();
        var sink = new AsstFileSink(_dir);
        var server = new AssistantServer(listener, sink);
        using var cts = new CancellationTokenSource();
        var runTask = server.RunAsync(cts.Token);

        // 第一个连接：只发 HELLO，让服务器进入等待 DATA 状态（活跃传输）
        var (ct1, st1) = MemoryAsstTransport.CreatePair();
        listener.Enqueue(st1);
        var hello1 = new AsstHello("busy-1", "a.bin", 1000, 4096, "");
        await ct1.WriteAsync(AsstFrame.Build((byte)AsstMessageType.Hello, AsstMessages.EncodeHello(hello1)));
        var offer1 = await AsstFrame.ReadFrameAsync(ct1); // 服务器回 OFFER(Accept)
        Assert.Equal((byte)AsstMessageType.Offer, offer1.Type);

        // 第二个连接：应收到 BUSY
        var (ct2, st2) = MemoryAsstTransport.CreatePair();
        listener.Enqueue(st2);
        await using var client2 = new AssistantClient(ct2);
        var hello2 = new AsstHello("busy-2", "b.bin", 1000, 4096, "");
        var result = await client2.SendAsync(hello2, ReadFrom(new byte[1000]));

        Assert.False(result.Ok);
        Assert.Contains("忙", result.Error);
        cts.Cancel();
        await runTask;
    }

    [Fact]
    public async Task HashMismatch_SendsFailedDone_AndCleansPartial()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        var sink = new AsstFileSink(_dir);
        var server = new AssistantServer(new MemoryAsstListener(), sink);
        var handleTask = server.HandleAsync(serverT, CancellationToken.None);

        var source = new byte[50_000];
        Random.Shared.NextBytes(source);
        var hello = new AsstHello("mismatch", "bad.bin", source.Length, 8192, "WRONGHASH");

        await using var client = new AssistantClient(clientT);
        var result = await client.SendAsync(hello, ReadFrom(source));

        await handleTask;

        Assert.False(result.Ok);
        Assert.Contains("校验失败", result.Error);
        Assert.False(File.Exists(Path.Combine(_dir, "bad.bin.btpart")));
        Assert.False(File.Exists(Path.Combine(_dir, "bad.bin")));
    }

    [Fact]
    public async Task Cancel_KeepsPartial_ForLaterResume()
    {
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        var sink = new AsstFileSink(_dir);
        var server = new AssistantServer(new MemoryAsstListener(), sink);
        var handleTask = server.HandleAsync(serverT, CancellationToken.None);

        var source = new byte[200_000];
        Random.Shared.NextBytes(source);
        var hello = new AsstHello("cancel-1", "keep.bin", source.Length, 8192, "");
        using var cts = new CancellationTokenSource();
        await using var client = new AssistantClient(clientT);
        var progressSeen = new TaskCompletionSource<long>();
        var sendTask = client.SendAsync(hello, ReadFrom(source), (sent, _) =>
        {
            if (sent > 0) progressSeen.TrySetResult(sent);
        }, cts.Token);

        // 等客户端收到至少一个 ACK（服务端已落盘）后再取消，保证半成品存在
        await progressSeen.Task;
        cts.Cancel();
        await Assert.ThrowsAsync<OperationCanceledException>(() => sendTask);
        await handleTask;

        Assert.True(File.Exists(Path.Combine(_dir, "keep.bin.btpart")));
        Assert.True(File.Exists(Path.Combine(_dir, "keep.bin.btpart.meta")));
        Assert.False(File.Exists(Path.Combine(_dir, "keep.bin")));
    }
}
