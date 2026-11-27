using BluetoothTransfer.Core.Client;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;
using BluetoothTransfer.Receiver;
using Xunit;

namespace BluetoothTransfer.Tests;

public class ReceiverAppTests : IDisposable
{
    private readonly string _dir;

    public ReceiverAppTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_recv_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch { }
    }

    [Fact]
    public void DefaultOptions_AskIsTrue()
    {
        // 安全加固：默认每次接收前询问，防止已配对设备直接落盘。
        Assert.True(new ReceiverOptions { SaveDir = _dir }.Ask);
    }

    [Fact]
    public async Task Run_ReceivesFile_SavesToDir()
    {
        var listener = new MemoryAsstListener();
        // 显式关闭询问，避免测试触发控制台 y/N 输入导致挂起。
        var app = new ReceiverApp(new ReceiverOptions { SaveDir = _dir, Ask = false }, listener);
        using var cts = new CancellationTokenSource();
        var runTask = app.RunAsync(cts.Token);

        var source = new byte[50_000];
        Random.Shared.NextBytes(source);
        var sourcePath = Path.Combine(_dir, "src.bin");
        await File.WriteAllBytesAsync(sourcePath, source);
        var hello = new AsstHello("r1", "data.bin", source.Length, 8192,
            await AsstHash.ComputeSha256Async(sourcePath));

        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        listener.Enqueue(serverT);
        await using var client = new AssistantClient(clientT);
        var result = await client.SendAsync(hello, (offset, buffer, ct) =>
        {
            var n = Math.Min(buffer.Length, source.Length - (int)offset);
            source.AsSpan((int)offset, n).CopyTo(buffer.Span);
            return Task.FromResult(n);
        });

        Assert.True(result.Ok);
        Assert.Equal(source, await File.ReadAllBytesAsync(Path.Combine(_dir, "data.bin")));
        cts.Cancel();
        await runTask;
    }
}
