using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// <see cref="ExternalSendInbox"/> 单测：二次实例 → 主实例发送载荷收件箱的原子写、排空、去重、上限、残留清理。
/// 纯 BCL 文件逻辑（不依赖 WPF/WinRT/Windows），Linux harness 与 Windows 测试工程均可直接运行。
/// 每个用例使用系统临时目录下的独立收件箱目录，结束后递归删除。
/// </summary>
public class ExternalSendInboxTests : IDisposable
{
    private readonly string _dir;
    private readonly string _inboxDir;
    private readonly ExternalSendInbox _inbox;

    public ExternalSendInboxTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), $"bt_inbox_{Guid.NewGuid():N}");
        _inboxDir = Path.Combine(_dir, "inbox");
        _inbox = new ExternalSendInbox(_inboxDir);
    }

    public void Dispose()
    {
        try
        {
            if (Directory.Exists(_dir)) Directory.Delete(_dir, recursive: true);
        }
        catch
        {
            // 测试清理失败不影响用例结论（临时目录由系统回收）
        }
    }

    // ---------------------------------------------------------------- 辅助

    private string CreateFile(string name, string content = "content")
    {
        var path = Path.Combine(_dir, name);
        Directory.CreateDirectory(_dir);
        File.WriteAllText(path, content);
        return path;
    }

    private string CreateDirectory(string name)
    {
        var path = Path.Combine(_dir, name);
        Directory.CreateDirectory(path);
        return path;
    }

    private static ShellSendItem FileItem(string path)
        => new() { Kind = ShellSendItemKind.File, Path = path };

    private static ShellSendItem FolderItem(string path)
        => new() { Kind = ShellSendItemKind.Folder, Path = path };

    private static ShellSendItem TextItem(string path, string text)
        => new() { Kind = ShellSendItemKind.Text, Path = path, Text = text };

    private string[] InboxFiles(string pattern = "*")
        => Directory.Exists(_inboxDir) ? Directory.GetFiles(_inboxDir, pattern) : Array.Empty<string>();

    // ---------------------------------------------------------------- 目录与写入

    [Fact]
    public void DefaultDirectory_IsUnderLocalAppData()
    {
        var expected = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "BluetoothTransfer",
            "inbox");

        Assert.Equal(Path.GetFullPath(expected), new ExternalSendInbox().InboxDirectory);
    }

    [Fact]
    public void InjectedDirectory_IsUsed()
    {
        Assert.Equal(Path.GetFullPath(_inboxDir), _inbox.InboxDirectory);
    }

    [Fact]
    public void TryWrite_WritesSingleGuidJson_WithoutTempLeftover()
    {
        var file = CreateFile("atomic.txt");

        Assert.True(_inbox.TryWrite(ShellSendRequest.FromItems(new[] { FileItem(file) }), out var error), error);

        var written = Assert.Single(InboxFiles("*.json"));
        Assert.Matches("^[0-9a-f]{32}\\.json$", Path.GetFileName(written));
        Assert.Empty(InboxFiles("*.tmp"));
    }

    [Fact]
    public async Task TryWrite_ThenDrain_RoundTripsItemsAndWarnings()
    {
        var file = CreateFile("a.txt");
        var folder = CreateDirectory("docs");
        var textFile = CreateFile("note.txt", "正文");
        var request = ShellSendRequest.FromItems(
            new[] { FileItem(file), FolderItem(folder), TextItem(textFile, "你好，待发送文本") },
            new[] { "解析阶段提示：一项已跳过" });

        Assert.True(_inbox.TryWrite(request, out var error), error);
        var drained = await _inbox.DrainAsync();

        Assert.Equal(3, drained.Count);
        Assert.Equal(ShellSendItemKind.File, drained.Items[0].Kind);
        Assert.Equal(file, drained.Items[0].Path);
        Assert.Equal(ShellSendItemKind.Folder, drained.Items[1].Kind);
        Assert.Equal(folder, drained.Items[1].Path);
        Assert.Equal(ShellSendItemKind.Text, drained.Items[2].Kind);
        Assert.Equal("你好，待发送文本", drained.Items[2].Text);
        Assert.Contains(drained.Warnings, w => w.Contains("解析阶段提示"));
        Assert.Empty(InboxFiles()); // 读出后全部删除
    }

    [Fact]
    public void TryWrite_RejectsEmptyRequest()
    {
        Assert.False(_inbox.TryWrite(ShellSendRequest.Empty, out var error));
        Assert.False(string.IsNullOrWhiteSpace(error));
    }

    [Fact]
    public async Task TryWrite_WarningsOnlyRequest_IsForwardedToMainInstance()
    {
        // 二次实例解析失败（如路径已失效）时没有发送项、只有提示：仍应转发给主实例展示，避免右键后静默无反应。
        var request = ShellSendRequest.FromItems(null, new[] { "路径不存在或不可访问，已忽略：C:\\missing.bin" });

        Assert.True(_inbox.TryWrite(request, out var error), error);
        var drained = await _inbox.DrainAsync();

        Assert.False(drained.HasItems);
        Assert.Contains(drained.Warnings, w => w.Contains("路径不存在"));
        Assert.Empty(InboxFiles());
    }

    [Fact]
    public void TryWrite_RejectsOversizedPayload()
    {
        var path = CreateFile("huge.txt");
        var hugeText = new string('中', 400 * 1024); // UTF-8 下约 1.2MB，超过单文件 1MB 上限
        var request = ShellSendRequest.FromItems(new[] { TextItem(path, hugeText) });

        Assert.False(_inbox.TryWrite(request, out var error));
        Assert.Contains("过大", error);
        Assert.Empty(InboxFiles("*.json"));
    }

    [Fact]
    public async Task TryWrite_WhenAllPathsMissing_ForwardsWarningOnlyPayload()
    {
        var missing = Path.Combine(_dir, "gone.bin");
        var request = ShellSendRequest.FromItems(new[] { FileItem(missing) });

        // 全部路径失效时不转发任何发送项，但把失败原因写进收件箱供主实例展示（右键后不静默）。
        Assert.True(_inbox.TryWrite(request, out var error), error);
        var drained = await _inbox.DrainAsync();

        Assert.False(drained.HasItems);
        Assert.Contains(drained.Warnings, w => w.Contains("未转发") || w.Contains("不存在"));
        Assert.Empty(InboxFiles());
    }

    [Fact]
    public async Task TryWrite_FiltersMissingPaths_WithWarning()
    {
        var existing = CreateFile("keep.bin");
        var missing = Path.Combine(_dir, "gone.bin");
        var request = ShellSendRequest.FromItems(new[] { FileItem(existing), FileItem(missing) });

        Assert.True(_inbox.TryWrite(request, out var error), error);
        var drained = await _inbox.DrainAsync();

        Assert.Single(drained.Items);
        Assert.Contains(drained.Warnings, w => w.Contains("未转发"));
    }

    // ---------------------------------------------------------------- 排空与安全

    [Fact]
    public async Task Drain_WhenInboxMissing_ReturnsEmpty()
    {
        var missingInbox = new ExternalSendInbox(Path.Combine(_dir, "not-created"));

        var drained = await missingInbox.DrainAsync();

        Assert.False(drained.HasItems);
        Assert.Empty(drained.Warnings);
    }

    [Fact]
    public async Task Drain_MergesMultipleRequests_AndDeduplicates()
    {
        var shared = CreateFile("shared.bin");
        var second = CreateFile("second.bin");
        Assert.True(_inbox.TryWrite(ShellSendRequest.FromItems(new[] { FileItem(shared) }), out var e1), e1);
        Assert.True(_inbox.TryWrite(ShellSendRequest.FromItems(new[] { FileItem(shared), FileItem(second) }), out var e2), e2);

        var drained = await _inbox.DrainAsync();

        Assert.Equal(2, drained.Count);
        Assert.Empty(InboxFiles());
    }

    [Fact]
    public async Task Drain_SkipsMissingPaths_AndLogs()
    {
        var file = CreateFile("will-delete.bin");
        Assert.True(_inbox.TryWrite(ShellSendRequest.FromItems(new[] { FileItem(file) }), out var error), error);
        File.Delete(file);

        var logs = new List<string>();
        var drained = await _inbox.DrainAsync(logs.Add);

        Assert.False(drained.HasItems);
        Assert.Contains(logs, l => l.Contains("已不存在"));
        Assert.Empty(InboxFiles());
    }

    [Fact]
    public async Task Drain_InvalidAndOversizedFiles_AreDiscardedAndLogged()
    {
        var file = CreateFile("valid.bin");
        Assert.True(_inbox.TryWrite(ShellSendRequest.FromItems(new[] { FileItem(file) }), out var error), error);
        File.WriteAllText(Path.Combine(_inboxDir, "0-invalid.json"), "{ this is not json");
        File.WriteAllText(Path.Combine(_inboxDir, "1-oversized.json"),
            new string('x', ExternalSendInbox.MaxJsonBytes + 16));

        var logs = new List<string>();
        var drained = await _inbox.DrainAsync(logs.Add);

        Assert.Single(drained.Items);
        Assert.Equal(file, drained.Items[0].Path);
        Assert.Contains(logs, l => l.Contains("解析失败"));
        Assert.Contains(logs, l => l.Contains("上限"));
        Assert.Empty(InboxFiles());
    }

    // ---------------------------------------------------------------- 过期残留清理

    [Fact]
    public async Task CleanResidue_RemovesOldJsonAndTemp_KeepsFresh()
    {
        var file = CreateFile("keep.bin");
        Assert.True(_inbox.TryWrite(ShellSendRequest.FromItems(new[] { FileItem(file) }), out var error), error);

        var oldJson = Path.Combine(_inboxDir, "old-residue.json");
        File.WriteAllText(oldJson, "{}");
        File.SetLastWriteTimeUtc(oldJson, DateTime.UtcNow - TimeSpan.FromDays(8));

        var oldTemp = Path.Combine(_inboxDir, "interrupted.json.tmp");
        File.WriteAllText(oldTemp, "partial");
        File.SetLastWriteTimeUtc(oldTemp, DateTime.UtcNow - TimeSpan.FromHours(2));

        var freshTemp = Path.Combine(_inboxDir, "writing.json.tmp");
        File.WriteAllText(freshTemp, "partial");

        var logs = new List<string>();
        var removed = _inbox.CleanResidue(log: logs.Add);

        Assert.Equal(2, removed);
        Assert.False(File.Exists(oldJson));
        Assert.False(File.Exists(oldTemp));
        Assert.True(File.Exists(freshTemp));
        Assert.Single(InboxFiles("*.json")); // 未过期的正常请求保留，等待排空
        Assert.Contains(logs, l => l.Contains("过期"));

        var drained = await _inbox.DrainAsync();
        Assert.Single(drained.Items);
    }

    [Fact]
    public void CleanResidue_WhenInboxMissing_ReturnsZero()
    {
        var missingInbox = new ExternalSendInbox(Path.Combine(_dir, "not-created"));

        Assert.Equal(0, missingInbox.CleanResidue());
    }
}
