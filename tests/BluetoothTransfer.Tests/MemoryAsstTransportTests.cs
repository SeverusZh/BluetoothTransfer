using System.Text;
using BluetoothTransfer.Core.IO;
using Xunit;

namespace BluetoothTransfer.Tests;

public class MemoryAsstTransportTests
{
    [Fact]
    public async Task Pair_Roundtrip()
    {
        var (a, b) = MemoryAsstTransport.CreatePair();
        var data = Encoding.UTF8.GetBytes("你好 蓝牙");
        await a.WriteAsync(data);

        var buffer = new byte[64];
        var n = await b.ReadAsync(buffer);

        Assert.Equal(data.Length, n);
        Assert.Equal(data, buffer.AsSpan(0, n).ToArray());
    }

    [Fact]
    public async Task Pair_LargePayload_Roundtrip()
    {
        var (a, b) = MemoryAsstTransport.CreatePair();
        var data = new byte[1_000_000];
        Random.Shared.NextBytes(data);
        await a.WriteAsync(data);

        using var ms = new MemoryStream();
        var buffer = new byte[32768];
        while (ms.Length < data.Length)
        {
            var n = await b.ReadAsync(buffer);
            if (n == 0) break;
            ms.Write(buffer, 0, n);
        }
        Assert.Equal(data, ms.ToArray());
    }

    [Fact]
    public async Task Broken_ReadReturnsZero()
    {
        var (a, b) = MemoryAsstTransport.CreatePair();
        a.Broken = true;
        var buffer = new byte[4];
        Assert.Equal(0, await a.ReadAsync(buffer));
    }

    [Fact]
    public async Task Listener_AcceptsQueuedConnection()
    {
        var listener = new MemoryAsstListener();
        var (a, b) = MemoryAsstTransport.CreatePair();
        listener.Enqueue(b);

        await using var accepted = await listener.AcceptAsync();
        await a.WriteAsync(new byte[] { 7 });
        var buffer = new byte[4];
        Assert.Equal(1, await accepted.ReadAsync(buffer));
        Assert.Equal(7, buffer[0]);
    }
}
