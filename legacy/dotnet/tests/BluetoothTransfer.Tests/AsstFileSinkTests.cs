using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AsstFileSinkTests : IDisposable
{
    private readonly string _dir;

    public AsstFileSinkTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_sink_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch { }
    }

    private static AsstHello Hello(string id = "t1", string name = "a.bin", long size = 5, string sha = "")
        => new(id, name, size, 32768, sha);

    /// <summary>计算给定字节内容的 SHA-256（十六进制），供期望哈希与落盘内容匹配。</summary>
    private static async Task<string> ShaOf(byte[] data)
    {
        var tmp = Path.Combine(Path.GetTempPath(), "bt_sha_" + Guid.NewGuid().ToString("N"));
        await File.WriteAllBytesAsync(tmp, data);
        try { return await AsstHash.ComputeSha256Async(tmp); }
        finally { try { File.Delete(tmp); } catch { } }
    }

    [Fact]
    public async Task OpenNew_WriteComplete_RenamesAndCleansMeta()
    {
        var sink = new AsstFileSink(_dir);
        var content = new byte[] { 1, 2, 3, 4, 5 };
        var hello = Hello(size: 5, sha: await ShaOf(content));
        Assert.Equal(0, await sink.OpenAsync(hello));

        await sink.WriteAsync(hello, 0, content);
        var done = await sink.CompleteAsync(hello);

        Assert.True(done.Ok);
        Assert.Equal(64, done.Hash.Length);
        Assert.Equal(content, await File.ReadAllBytesAsync(Path.Combine(_dir, "a.bin")));
        Assert.False(File.Exists(Path.Combine(_dir, "a.bin.btpart")));
        Assert.False(File.Exists(Path.Combine(_dir, "a.bin.btpart.meta")));
    }

    [Fact]
    public async Task OpenAgain_SameHello_ReturnsResumeOffset()
    {
        var sink = new AsstFileSink(_dir);
        var hello = Hello(size: 5);
        await sink.OpenAsync(hello);
        await sink.WriteAsync(hello, 0, new byte[] { 1, 2, 3 });

        Assert.Equal(3, await sink.OpenAsync(hello));
    }

    [Fact]
    public async Task OpenAgain_DifferentTransferId_StartsOver()
    {
        var sink = new AsstFileSink(_dir);
        var hello = Hello(size: 5);
        await sink.OpenAsync(hello);
        await sink.WriteAsync(hello, 0, new byte[] { 1, 2, 3 });

        Assert.Equal(0, await sink.OpenAsync(Hello(id: "other", size: 5)));
    }

    [Fact]
    public async Task Complete_HashMismatch_DeletesPartial()
    {
        var sink = new AsstFileSink(_dir);
        var hello = Hello(size: 5, sha: "WRONG");
        await sink.OpenAsync(hello);
        await sink.WriteAsync(hello, 0, new byte[] { 1, 2, 3, 4, 5 });

        var done = await sink.CompleteAsync(hello);

        Assert.False(done.Ok);
        Assert.False(File.Exists(Path.Combine(_dir, "a.bin.btpart")));
        Assert.False(File.Exists(Path.Combine(_dir, "a.bin.btpart.meta")));
    }

    [Fact]
    public async Task Complete_EmptyExpectedSha256_ReturnsFailedDone()
    {
        var sink = new AsstFileSink(_dir);
        var hello = Hello(size: 5, sha: "");
        await sink.OpenAsync(hello);
        await sink.WriteAsync(hello, 0, new byte[] { 1, 2, 3, 4, 5 });

        var done = await sink.CompleteAsync(hello);

        Assert.False(done.Ok);
        Assert.Equal(64, done.Hash.Length);
        // 纵深防御：空期望哈希不放行，半成品也不应被改名为正式文件
        Assert.False(File.Exists(Path.Combine(_dir, "a.bin")));
    }

    [Fact]
    public async Task Complete_NameConflict_AutoRenames()
    {
        var sink = new AsstFileSink(_dir);
        await File.WriteAllBytesAsync(Path.Combine(_dir, "a.bin"), new byte[] { 9 });
        var content = new byte[] { 1, 2, 3, 4, 5 };
        var hello = Hello(size: 5, sha: await ShaOf(content));
        await sink.OpenAsync(hello);
        await sink.WriteAsync(hello, 0, content);
        await sink.CompleteAsync(hello);

        Assert.Equal(content, await File.ReadAllBytesAsync(Path.Combine(_dir, "a (1).bin")));
    }

    [Fact]
    public void Sanitize_ReplacesInvalidChars()
    {
        Assert.Equal("a_b_c.txt", AsstFileSink.Sanitize("a/b:c.txt"));
        Assert.Equal("file", AsstFileSink.Sanitize("  "));
    }

    [Fact]
    public void CleanStale_RemovesOldPartials()
    {
        var sink = new AsstFileSink(_dir);
        var old = Path.Combine(_dir, "old.bin.btpart");
        File.WriteAllText(old, "x");
        File.SetLastWriteTime(old, DateTime.Now.AddDays(-8));
        File.WriteAllText(Path.Combine(_dir, "new.bin.btpart"), "y");

        var removed = AsstFileSink.CleanStale(_dir, TimeSpan.FromDays(7));

        Assert.Equal(1, removed);
        Assert.False(File.Exists(old));
        Assert.True(File.Exists(Path.Combine(_dir, "new.bin.btpart")));
    }
}
