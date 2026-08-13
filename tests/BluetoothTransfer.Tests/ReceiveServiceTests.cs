using BluetoothTransfer.Core.Client;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using System.Security.Cryptography;
using Xunit;

namespace BluetoothTransfer.Tests;

public class ReceiveServiceTests : IDisposable
{
    private readonly string _dir;
    private readonly StorageService _storage;

    public ReceiveServiceTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_recvsvc_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
        _storage = new StorageService(Path.Combine(_dir, "test.db"));
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
    public async Task Start_ReceivesFile_WritesRecvRecordAndFiresCompleted()
    {
        var listener = new MemoryAsstListener();
        var svc = new ReceiveService(_storage, new EventBus(), listener);
        var completed = new TaskCompletionSource<string>();
        svc.Completed += name => completed.TrySetResult(name);
        using var cts = new CancellationTokenSource();

        await svc.StartAsync(_dir, ask: false, cts.Token);

        var source = new byte[40_000];
        Random.Shared.NextBytes(source);
        var sha = Convert.ToHexString(SHA256.HashData(source)).ToLowerInvariant();
        var hello = new AsstHello("r1", "recv.bin", source.Length, 8192, sha);
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        listener.Enqueue(serverT);
        await using var client = new AssistantClient(clientT);

        var result = await client.SendAsync(hello, ReadFrom(source));

        Assert.True(result.Ok);
        Assert.Equal("recv.bin", await completed.Task.WaitAsync(TimeSpan.FromSeconds(10)));
        Assert.Equal(source, await File.ReadAllBytesAsync(Path.Combine(_dir, "recv.bin")));
        var record = Assert.Single(_storage.GetRecords());
        Assert.Equal(TransferConst.DirRecv, record.Direction);
        Assert.Equal(TransferConst.ChannelAssistant, record.Channel);
        Assert.Equal(TransferConst.StatusOk, record.Status);
        Assert.Equal("recv.bin", record.Name);

        await svc.StopAsync();
    }

    [Fact]
    public async Task Start_RecordsPeerAddressAndName()
    {
        _storage.UpsertDevice(new DeviceInfo
        {
            Addr = "00:A7:60:50:75:04",
            Name = "对端电脑"
        });
        var listener = new MemoryAsstListener();
        var svc = new ReceiveService(_storage, new EventBus(), listener);
        var completed = new TaskCompletionSource<string>();
        svc.Completed += name => completed.TrySetResult(name);
        using var cts = new CancellationTokenSource();
        await svc.StartAsync(_dir, ask: false, cts.Token);

        var source = new byte[10_000];
        Random.Shared.NextBytes(source);
        var sha = Convert.ToHexString(SHA256.HashData(source)).ToLowerInvariant();
        var hello = new AsstHello("r2", "peer.bin", source.Length, 8192, sha);
        var (clientT, serverT) = MemoryAsstTransport.CreatePair();
        serverT.RemoteAddress = "00:A7:60:50:75:04";
        listener.Enqueue(serverT);
        await using var client = new AssistantClient(clientT);

        var result = await client.SendAsync(hello, ReadFrom(source));

        Assert.True(result.Ok);
        Assert.Equal("peer.bin", await completed.Task.WaitAsync(TimeSpan.FromSeconds(10)));
        var record = Assert.Single(_storage.GetRecords());
        Assert.Equal("00:a7:60:50:75:04", record.PeerAddr);
        Assert.Equal("对端电脑", record.PeerName);

        await svc.StopAsync();
    }
}
