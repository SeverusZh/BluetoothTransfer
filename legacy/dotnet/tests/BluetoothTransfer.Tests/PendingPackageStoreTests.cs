using System.IO.Compression;
using System.Text;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class PendingPackageStoreTests : IDisposable
{
    private readonly string _dir;
    private readonly string _packageDir;
    private readonly PendingPackageStore _store;

    public PendingPackageStoreTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), $"bt_pkg_{Guid.NewGuid():N}");
        Directory.CreateDirectory(_dir);
        _packageDir = Path.Combine(_dir, "pending");
        _store = new PendingPackageStore(_packageDir);
    }

    public void Dispose()
    {
        try { if (Directory.Exists(_dir)) Directory.Delete(_dir, recursive: true); } catch { }
    }

    private string WriteFile(string name, string content)
    {
        var path = Path.Combine(_dir, name);
        File.WriteAllText(path, content);
        return path;
    }

    [Fact]
    public async Task CreateZipFromFile_KeepsOriginalEntryName()
    {
        var source = WriteFile("report.txt", "hello zip");

        var package = await _store.CreateZipFromFileAsync(source);

        Assert.True(File.Exists(package));
        Assert.True(_store.IsOwned(package));
        using var archive = ZipFile.OpenRead(package);
        var entry = Assert.Single(archive.Entries);
        Assert.Equal("report.txt", entry.FullName);
    }

    [Fact]
    public async Task CreateZipFromFolder_ExcludesBaseDirectory()
    {
        var folder = Path.Combine(_dir, "docs");
        Directory.CreateDirectory(folder);
        await File.WriteAllTextAsync(Path.Combine(folder, "a.txt"), "a");

        var package = await _store.CreateZipFromFolderAsync(folder);

        using var archive = ZipFile.OpenRead(package);
        Assert.Equal("a.txt", Assert.Single(archive.Entries).FullName);
    }

    [Fact]
    public async Task CreateTextFile_WritesUtf8WithoutBom()
    {
        var package = await _store.CreateTextFileAsync("中文内容", "bt-note.txt");

        var bytes = await File.ReadAllBytesAsync(package);
        Assert.False(bytes.Length >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF);
        Assert.Equal("中文内容", Encoding.UTF8.GetString(bytes));
    }

    [Fact]
    public void Delete_OnlyTouchesOwnedPackages()
    {
        var outside = WriteFile("user-file.bin", "important");

        Assert.False(_store.IsOwned(outside));
        Assert.False(_store.Delete(outside));
        Assert.True(File.Exists(outside)); // 绝不误删用户文件
    }

    [Fact]
    public async Task Delete_RemovesOwnedPackage()
    {
        var package = await _store.CreateTextFileAsync("x", "n.txt");

        Assert.True(_store.Delete(package));
        Assert.False(File.Exists(package));
    }

    [Fact]
    public async Task CleanOrphans_DeletesOldUnreferenced_KeepsReferenced()
    {
        var referenced = await _store.CreateTextFileAsync("keep", "keep.txt");
        var orphan = await _store.CreateTextFileAsync("drop", "drop.txt");
        var fresh = await _store.CreateTextFileAsync("new", "new.txt");
        var old = DateTime.UtcNow - TimeSpan.FromDays(8);
        File.SetLastWriteTimeUtc(orphan, old);
        File.SetLastWriteTimeUtc(referenced, old); // 旧但被引用：保留
        // fresh 保持新时间：即便未被引用也不清理（避免误删正在写入的产物）

        var removed = _store.CleanOrphans(new[] { referenced }, TransferJournal.DefaultTtl);

        Assert.Equal(1, removed);
        Assert.True(File.Exists(referenced));
        Assert.False(File.Exists(orphan));
        Assert.True(File.Exists(fresh));
    }
}
