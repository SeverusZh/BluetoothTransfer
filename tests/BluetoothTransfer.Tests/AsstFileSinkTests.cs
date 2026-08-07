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

    [Fact]
    public async Task OpenNew_WriteComplete_RenamesAndCleansMeta()
    {
        var sink = new AsstFileSink(_dir);
        var hello = Hello(size: 5);
        Assert.Equal(0, await sink.OpenAsync(hello));

        await sink.WriteAsync(hello, 0, new byte[] { 1, 2, 3, 4, 5 });
        var done = await sink.CompleteAsync(hello);

        Assert.True(done.Ok);
        Assert.Equal(64, done.Hash.Length);
        Assert.Equal(new byte[] { 1, 2, 3, 4, 5 }, await File.ReadAllBytesAsync(Path.Combine(_dir, "a.bin")));
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
    public async Task Complete_NameConflict_AutoRenames()
    {
        var sink = new AsstFileSink(_dir);
        await File.WriteAllBytesAsync(Path.Combine(_dir, "a.bin"), new byte[] { 9 });
        var hello = Hello(size: 5);
        await sink.OpenAsync(hello);
        await sink.WriteAsync(hello, 0, new byte[] { 1, 2, 3, 4, 5 });
        await sink.CompleteAsync(hello);

        Assert.Equal(new byte[] { 1, 2, 3, 4, 5 }, await File.ReadAllBytesAsync(Path.Combine(_dir, "a (1).bin")));
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
