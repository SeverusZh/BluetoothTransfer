using System.Text;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// <see cref="ShellSendRequest"/> / <see cref="ShellTextFileReader"/> / <see cref="ShellSendSyntax"/> 单测。
/// 纯 BCL 逻辑（不依赖 WPF/WinRT/注册表），Linux harness 与 Windows 测试工程均可直接运行。
/// 用例统一使用系统临时目录下的随机子目录，互不干扰；文件名/内容覆盖空格、中文与 BOM 变体。
/// </summary>
public class ShellSendRequestTests : IDisposable
{
    private readonly string _dir;

    public ShellSendRequestTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), $"bt_shell_send_{Guid.NewGuid():N}");
        Directory.CreateDirectory(_dir);
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

    private string CreateFile(string name, string content = "content")
    {
        var path = Path.Combine(_dir, name);
        File.WriteAllText(path, content);
        return path;
    }

    private string CreateFileBytes(string name, byte[] bytes)
    {
        var path = Path.Combine(_dir, name);
        File.WriteAllBytes(path, bytes);
        return path;
    }

    private string CreateDirectory(string name)
    {
        var path = Path.Combine(_dir, name);
        Directory.CreateDirectory(path);
        return path;
    }

    // ---------------------------------------------------------------- 基础解析

    [Fact]
    public void Parse_NoArguments_ReturnsEmpty()
    {
        foreach (var args in new[] { null, Array.Empty<string>(), new[] { "" } })
        {
            var request = ShellSendRequest.Parse(args);
            Assert.False(request.HasItems);
            Assert.Equal(0, request.Count);
            Assert.Empty(request.Warnings);
        }
    }

    [Fact]
    public void Parse_UnknownArguments_ReturnsEmptyRequest_SoGuiStillStarts()
    {
        var request = ShellSendRequest.Parse(new[] { "--unknown-option", "other-arg" });

        Assert.False(request.HasItems);
        Assert.Contains(request.Warnings, w => w.Contains("--unknown-option"));
    }

    [Fact]
    public void Parse_SendFileAndFolder_ClassifiesKindsAndNormalizesPaths()
    {
        var file = CreateFile("报告 v1.txt");
        var folder = CreateDirectory("资料");

        var request = ShellSendRequest.Parse(new[] { "--send", file, folder });

        Assert.Equal(2, request.Count);
        Assert.Equal(ShellSendItemKind.File, request.Items[0].Kind);
        Assert.Equal(Path.GetFullPath(file), request.Items[0].Path);
        Assert.Equal(ShellSendItemKind.Folder, request.Items[1].Kind);
        Assert.Equal(Path.GetFullPath(folder), request.Items[1].Path);
    }

    [Fact]
    public void Parse_DuplicatePaths_AreDeduplicated()
    {
        var file = CreateFile("dup.txt");

        var request = ShellSendRequest.Parse(new[]
        {
            "--send", file, file, Path.Combine(_dir, ".", "dup.txt")
        });

        Assert.Single(request.Items);
    }

    [Fact]
    public void Parse_DuplicatePaths_DifferingOnlyByCase_AreDeduplicatedOnWindows()
    {
        // Linux 文件系统区分大小写，构造不出「同一文件的大小写变体」；该分支在 Windows 上验证。
        if (!OperatingSystem.IsWindows()) return;

        var file = CreateFile("CaseOnly.txt");
        var request = ShellSendRequest.Parse(new[] { "--send", file, file.ToUpperInvariant() });

        Assert.Single(request.Items);
    }

    [Fact]
    public void Parse_MissingPath_IsSkippedWithWarning()
    {
        var missing = Path.Combine(_dir, "not-exists.txt");

        var request = ShellSendRequest.Parse(new[] { "--send", missing });

        Assert.False(request.HasItems);
        Assert.Contains(request.Warnings, w => w.Contains("不存在"));
    }

    [Fact]
    public void Parse_MoreThanMaxItems_IsCappedWithWarning()
    {
        var paths = new List<string> { "--send" };
        for (var i = 0; i < ShellSendRequest.MaxItems + 5; i++)
            paths.Add(CreateFile($"many_{i}.bin"));

        var request = ShellSendRequest.Parse(paths);

        Assert.Equal(ShellSendRequest.MaxItems, request.Count);
        Assert.Contains(request.Warnings, w => w.Contains("最多处理"));
    }

    [Fact]
    public void Parse_SendWithoutPath_WarnsAndKeepsGoing()
    {
        var request = ShellSendRequest.Parse(new[] { "--send", "--foo" });

        Assert.False(request.HasItems);
        Assert.Contains(request.Warnings, w => w.Contains("缺少路径参数"));
    }

    // ---------------------------------------------------------------- --send-text 文本读取

    [Fact]
    public void Parse_SendText_ReadsUtf8WithoutBom()
    {
        var file = CreateFile("note.txt", "你好，蓝牙传输！");

        var request = ShellSendRequest.Parse(new[] { "--send-text", file });

        var item = Assert.Single(request.Items);
        Assert.Equal(ShellSendItemKind.Text, item.Kind);
        Assert.Equal(file, item.Path);
        Assert.Equal("你好，蓝牙传输！", item.Text);
        Assert.Empty(request.Warnings);
    }

    [Fact]
    public void Parse_SendText_StripsUtf8Bom()
    {
        var file = CreateFileBytes("bom-utf8.txt",
            new byte[] { 0xEF, 0xBB, 0xBF }.Concat(Encoding.UTF8.GetBytes("BOM 文本")).ToArray());

        var request = ShellSendRequest.Parse(new[] { "--send-text", file });

        var item = Assert.Single(request.Items);
        Assert.Equal(ShellSendItemKind.Text, item.Kind);
        Assert.Equal("BOM 文本", item.Text);
    }

    [Fact]
    public void Parse_SendText_DetectsUtf16LittleEndianWithBom()
    {
        var bytes = Encoding.Unicode.GetPreamble()
            .Concat(Encoding.Unicode.GetBytes("记事本 UTF-16 文本")).ToArray();
        var file = CreateFileBytes("utf16le.txt", bytes);

        var request = ShellSendRequest.Parse(new[] { "--send-text", file });

        var item = Assert.Single(request.Items);
        Assert.Equal(ShellSendItemKind.Text, item.Kind);
        Assert.Equal("记事本 UTF-16 文本", item.Text);
    }

    [Fact]
    public void Parse_SendText_DetectsUtf16WithoutBom()
    {
        // 无 BOM 的 UTF-16 LE（旧版记事本“Unicode”格式）：靠 0x00 的奇偶分布识别。
        var file = CreateFileBytes("utf16le-nobom.txt", Encoding.Unicode.GetBytes("Hello 世界"));

        var request = ShellSendRequest.Parse(new[] { "--send-text", file });

        var item = Assert.Single(request.Items);
        Assert.Equal(ShellSendItemKind.Text, item.Kind);
        Assert.Equal("Hello 世界", item.Text);
    }

    [Fact]
    public void Parse_SendText_InvalidUtf8_FallsBackToFileWithWarning()
    {
        // 0xC3 0x28 / 0x80 / 0xFF 不是合法 UTF-8 序列（二进制或 ANSI 编码文件的典型表现）；
        // 同时刻意不含 0x00，避免命中无 BOM UTF-16 的启发式判定。
        var file = CreateFileBytes("binary.txt", new byte[] { 0x01, 0xC3, 0x28, 0x80, 0xFF, 0x7F });

        var request = ShellSendRequest.Parse(new[] { "--send-text", file });

        var item = Assert.Single(request.Items);
        Assert.Equal(ShellSendItemKind.File, item.Kind);
        Assert.Equal(file, item.Path);
        Assert.Contains(request.Warnings, w => w.Contains("按文件发送"));
    }

    [Fact]
    public void Parse_SendText_TooLarge_FallsBackToFileWithWarning()
    {
        var text = new string('a', ShellTextFileReader.MaxTextBytes + 1024);
        var file = CreateFile("too-large.txt", text);

        var request = ShellSendRequest.Parse(new[] { "--send-text", file });

        var item = Assert.Single(request.Items);
        Assert.Equal(ShellSendItemKind.File, item.Kind);
        Assert.Contains(request.Warnings, w => w.Contains("按文件发送") && w.Contains("过大"));
    }

    [Fact]
    public void Parse_SendText_MissingFile_IsSkippedWithWarning()
    {
        var missing = Path.Combine(_dir, "missing.txt");

        var request = ShellSendRequest.Parse(new[] { "--send-text", missing });

        Assert.False(request.HasItems);
        Assert.Contains(request.Warnings, w => w.Contains("文本文件不存在"));
    }

    [Fact]
    public void Parse_SendText_WithoutFileArgument_Warns()
    {
        var request = ShellSendRequest.Parse(new[] { "--send-text" });

        Assert.False(request.HasItems);
        Assert.Contains(request.Warnings, w => w.Contains("缺少文本文件参数"));
    }

    [Fact]
    public void ShellTextFileReader_RejectsEmptyFile()
    {
        var file = CreateFile("empty.txt", "");

        Assert.False(ShellTextFileReader.TryReadText(file, out _, out var error));
        Assert.Contains("为空", error);
    }

    // ---------------------------------------------------------------- 合并

    [Fact]
    public void Merge_DeduplicatesAcrossRequests()
    {
        var file = CreateFile("merge.txt");
        var other = CreateFile("other.txt");
        var first = ShellSendRequest.FromItems(new[]
        {
            new ShellSendItem { Kind = ShellSendItemKind.File, Path = file }
        });
        var second = ShellSendRequest.FromItems(new[]
        {
            new ShellSendItem { Kind = ShellSendItemKind.File, Path = Path.Combine(_dir, ".", "merge.txt") },
            new ShellSendItem { Kind = ShellSendItemKind.File, Path = other }
        }, new[] { "第二条提示" });

        var merged = ShellSendRequest.Merge(new[] { first, second });

        Assert.Equal(2, merged.Count);
        Assert.Contains(merged.Warnings, w => w == "第二条提示");
    }

    [Fact]
    public void Merge_CapsAtMaxItems()
    {
        var first = ShellSendRequest.FromItems(Enumerable.Range(0, 60)
            .Select(i => new ShellSendItem { Kind = ShellSendItemKind.File, Path = Path.Combine(_dir, $"a_{i}.bin") }));
        var second = ShellSendRequest.FromItems(Enumerable.Range(0, 60)
            .Select(i => new ShellSendItem { Kind = ShellSendItemKind.File, Path = Path.Combine(_dir, $"b_{i}.bin") }));

        var merged = ShellSendRequest.Merge(new[] { first, second });

        Assert.Equal(ShellSendRequest.MaxItems, merged.Count);
        Assert.Contains(merged.Warnings, w => w.Contains("最多处理"));
    }

    [Fact]
    public void Merge_NullRequests_ReturnsEmpty()
    {
        Assert.False(ShellSendRequest.Merge(null).HasItems);
        Assert.False(ShellSendRequest.Merge(new ShellSendRequest?[] { null, ShellSendRequest.Empty }).HasItems);
    }

    // ---------------------------------------------------------------- Shell 命令模板（注册表 command 字符串）

    [Fact]
    public void ShellSendSyntax_BuildsQuotedCommandLines()
    {
        const string exe = @"C:\Program Files\蓝牙传输\BluetoothTransfer.exe";

        Assert.Equal($"\"{exe}\" --send \"%1\"", ShellSendSyntax.BuildSendCommand(exe, "%1"));
        Assert.Equal($"\"{exe}\" --send \"%V\"", ShellSendSyntax.BuildSendCommand(exe, "%V"));
        Assert.Equal($"\"{exe}\" --send-text \"%1\"", ShellSendSyntax.BuildSendTextCommand(exe, "%1"));
        Assert.Equal($"\"{exe}\",0", ShellSendSyntax.BuildIconValue(exe));
        Assert.Equal("--send", ShellSendSyntax.SendToArguments);
        Assert.Equal("--send", ShellSendSyntax.SendOption);
        Assert.Equal("--send-text", ShellSendSyntax.SendTextOption);
        Assert.Equal("蓝牙传输.lnk", ShellSendSyntax.SendToShortcutFileName);
    }

    [Fact]
    public void ShellSendSyntax_QuoteArgument_WrapsWholeValue()
    {
        Assert.Equal("\"C:\\a b\\中文 文件.txt\"", ShellSendSyntax.QuoteArgument("C:\\a b\\中文 文件.txt"));
        Assert.Throws<ArgumentNullException>(() => ShellSendSyntax.QuoteArgument(null!));
    }
}
