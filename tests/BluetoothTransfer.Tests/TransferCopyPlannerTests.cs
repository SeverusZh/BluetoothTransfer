using System.IO;
using System.Text;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// TransferCopyPlanner 单元测试：纯文件系统 + 纯逻辑（不依赖 WPF / 剪贴板，可在跨平台 harness 运行）。
/// 覆盖：文本记录（UTF-8、BOM、UTF-16、大小上限）、图片/普通文件/文件夹分类、
/// 文件缺失与无本地路径的失败原因、单文件「复制内容」判定与“其它格式回退复制文件本身”的语义。
/// </summary>
public class TransferCopyPlannerTests : IDisposable
{
    private readonly string _dir;

    public TransferCopyPlannerTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_copyplan_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch { }
    }

    private string WriteBytes(string name, byte[] bytes)
    {
        var path = Path.Combine(_dir, name);
        File.WriteAllBytes(path, bytes);
        return path;
    }

    private string WriteText(string name, string text, Encoding encoding)
    {
        var path = Path.Combine(_dir, name);
        File.WriteAllText(path, text, encoding);
        return path;
    }

    private string WriteSizedFile(string name, long length)
    {
        // SetLength 生成指定大小的稀疏文件，避免为“大小上限”用例写入几十 MB 真实数据。
        var path = Path.Combine(_dir, name);
        using var fs = new FileStream(path, FileMode.CreateNew, FileAccess.Write);
        fs.SetLength(length);
        return path;
    }

    // ---------------------------------------------------------------- 通用复制：文本记录

    [Fact]
    public void PlanCopy_TextRecord_ReadsUtf8Text()
    {
        var path = WriteText("note.txt", "你好，蓝牙传输", new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "note.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Text, plan.Kind);
        Assert.Equal("你好，蓝牙传输", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecord_Utf8Bom_StripsBom()
    {
        var path = WriteText("bom.txt", "带 BOM 的文本", new UTF8Encoding(true));
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "bom.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal("带 BOM 的文本", plan.Payload);
        // 必须用序数比较（Ordinal）：xUnit 的双参数字符串 DoesNotContain 走 CurrentCulture，
        // 而 zh-CN 等排序规则把 U+FEFF 视为可忽略字符（"带 BOM 的文本".IndexOf("\uFEFF",
        // StringComparison.CurrentCulture) == 0），会让本断言在中文系统上假失败。
        Assert.DoesNotContain("\uFEFF", plan.Payload, StringComparison.Ordinal);
    }

    [Fact]
    public void PlanCopy_TextRecord_Utf16LeBom_Decodes()
    {
        var path = WriteText("utf16.txt", "UTF-16 中文内容", Encoding.Unicode);
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "utf16.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal("UTF-16 中文内容", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecord_Utf16BeBom_Decodes()
    {
        // UTF-16 大端 BOM（FE FF）+ 大端编码的 "Hi"：必须走 BigEndianUnicode 分支，不能被当成 UTF-16 LE。
        var path = WriteBytes("utf16be.txt", new byte[] { 0xFE, 0xFF, 0x00, 0x48, 0x00, 0x69 });
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "utf16be.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal("Hi", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecord_Utf32LeBom_Decodes()
    {
        // UTF-32 小端 BOM（FF FE 00 00）+ 小端编码的 "Hi"：FF FE 前缀不能被误判为 UTF-16 LE。
        var path = WriteBytes("utf32le.txt",
            new byte[] { 0xFF, 0xFE, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x69, 0x00, 0x00, 0x00 });
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "utf32le.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal("Hi", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecord_Utf32BeBom_Decodes()
    {
        // UTF-32 大端 BOM（00 00 FE FF）+ 大端编码的 "Hi"：必须用大端解码器，
        // 若误用默认小端的 Encoding.UTF32 会解成乱码（回归防护）。
        var path = WriteBytes("utf32be.txt",
            new byte[] { 0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, 0x48, 0x00, 0x00, 0x00, 0x69 });
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "utf32be.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal("Hi", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecord_NonUtf8Bytes_FallsBackToLatin1()
    {
        // 非 UTF-8 字节（这里模拟 GBK 的「你好」）：严格 UTF-8 解码失败后按 Latin1 逐字节映射，
        // 行为跨平台确定；.NET 8 的 Encoding.Default 是 UTF-8，不能用来还原 ANSI/GBK 旧文本。
        var path = WriteBytes("legacy.txt", new byte[] { 0xC4, 0xE3, 0xBA, 0xC3 });
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "legacy.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal("\u00C4\u00E3\u00BA\u00C3", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecord_FileMissing_FailsWithChineseReason()
    {
        var record = new TransferRecord
        {
            Type = TransferConst.TypeText,
            Name = "gone.txt",
            LocalPath = Path.Combine(_dir, "gone.txt")
        };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.None, plan.Kind);
        Assert.Contains("不存在", plan.Error);
    }

    [Fact]
    public void PlanCopy_NoLocalPath_FailsWithChineseReason()
    {
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "旧记录", LocalPath = "" };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("本地文件路径", plan.Error);
    }

    [Fact]
    public void PlanCopy_TextOverLimit_FailsWithoutReading()
    {
        var path = WriteText("big.txt", new string('x', 128), new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "big.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record, maxTextBytes: 32);

        Assert.False(plan.IsSuccess);
        Assert.Contains("过大", plan.Error);
    }

    // ---------------------------------------------------------------- 通用复制：schema v2 正文（Content）

    [Fact]
    public void PlanCopy_TextRecordWithContent_UsesContentWithoutReadingFile()
    {
        // 发送文本记录：临时 .txt 已被清理，但仍可用入库正文复制（本次数据模型的核心目的）。
        var record = new TransferRecord
        {
            Type = TransferConst.TypeText,
            Name = "bt-note.txt",
            LocalPath = Path.Combine(_dir, "已被清理的临时文件.txt"),
            Content = "记录里的正文"
        };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Text, plan.Kind);
        Assert.Equal("记录里的正文", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecordWithContent_PrefersContentOverLocalFile()
    {
        var path = WriteText("local.txt", "文件里的旧内容", new UTF8Encoding(false));
        var record = new TransferRecord
        {
            Type = TransferConst.TypeText,
            Name = "local.txt",
            LocalPath = path,
            Content = "记录里的新内容"
        };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.Equal(TransferCopyKind.Text, plan.Kind);
        Assert.Equal("记录里的新内容", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecordWithContent_OverLimitFailsWithoutCopying()
    {
        var record = new TransferRecord
        {
            Type = TransferConst.TypeText,
            Name = "huge.txt",
            Content = new string('正', 64)
        };

        var plan = TransferCopyPlanner.PlanCopy(record, maxTextBytes: 32);

        Assert.False(plan.IsSuccess);
        Assert.Contains("过大", plan.Error);
    }

    [Fact]
    public void PlanCopy_TextRecordWithoutContent_FallsBackToLocalPathFile()
    {
        // 旧库/旧记录没有 content 列：保持既有行为，读取 LocalPath 指向的文本文件。
        var path = WriteText("legacy.txt", "旧记录正文", new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "legacy.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Text, plan.Kind);
        Assert.Equal("旧记录正文", plan.Payload);
    }

    [Fact]
    public void PlanCopy_TextRecordWithoutContentAndMissingFile_GivesContentHint()
    {
        // 正文超限未入库（或旧版记录）且临时文件已被清理：提示中要说明“未保存正文”，而不是只说文件不存在。
        var record = new TransferRecord
        {
            Type = TransferConst.TypeText,
            Name = "超长文本.txt",
            LocalPath = Path.Combine(_dir, "not-exists.txt")
        };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("本地文件已不存在", plan.Error);
        Assert.Contains("未保存正文", plan.Error);
        Assert.Contains("上限", plan.Error);
    }

    [Fact]
    public void PlanCopy_FolderRecordWithMissingDirectory_FailsWithChineseReason()
    {
        var record = new TransferRecord
        {
            Type = TransferConst.TypeFolder,
            Name = "docs.zip",
            LocalPath = Path.Combine(_dir, "已删除的文件夹")
        };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("不存在", plan.Error);
    }

    // ---------------------------------------------------------------- 通用复制：文件 / 图片 / 文件夹

    [Fact]
    public void PlanCopy_ImageFileRecord_UsesImageKind()
    {
        var path = WriteBytes("photo.png", new byte[] { 0x89, 0x50, 0x4E, 0x47 });
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "photo.png", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Image, plan.Kind);
        Assert.Equal(Path.GetFullPath(path), plan.Payload);
    }

    [Fact]
    public void PlanCopy_PlainFileRecord_UsesFileKind()
    {
        var path = WriteBytes("data.bin", new byte[] { 1, 2, 3 });
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "data.bin", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.File, plan.Kind);
        Assert.Equal(Path.GetFullPath(path), plan.Payload);
    }

    [Fact]
    public void PlanCopy_TxtFileRecord_StillUsesFileKind()
    {
        // 用户确认的语义：通用复制只有 Type=text 的记录才复制文本；.txt 文件记录本身按「文件」进剪贴板。
        var path = WriteText("plain.txt", "文件内容", new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "plain.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.Equal(TransferCopyKind.File, plan.Kind);
        Assert.Equal(Path.GetFullPath(path), plan.Payload);
    }

    [Fact]
    public void PlanCopy_FolderRecord_UsesFileKind()
    {
        var folder = Path.Combine(_dir, "folder");
        Directory.CreateDirectory(folder);
        var record = new TransferRecord { Type = TransferConst.TypeFolder, Name = "folder", LocalPath = folder };

        var plan = TransferCopyPlanner.PlanCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.File, plan.Kind);
        Assert.Equal(Path.GetFullPath(folder), plan.Payload);
    }

    [Fact]
    public void PlanCopy_NullRecord_FailsWithChineseReason()
    {
        var plan = TransferCopyPlanner.PlanCopy(null);

        Assert.False(plan.IsSuccess);
        Assert.Contains("未指定", plan.Error);
    }

    // ---------------------------------------------------------------- 单文件专属「复制内容」

    [Fact]
    public void PlanContentCopy_TextFile_ReadsContent()
    {
        var path = WriteText("content.log", "日志内容", new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "content.log", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Text, plan.Kind);
        Assert.Equal("日志内容", plan.Payload);
    }

    [Fact]
    public void PlanContentCopy_ImageFile_UsesImageKind()
    {
        var path = WriteBytes("pic.jpg", new byte[] { 0xFF, 0xD8, 0xFF });
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "pic.jpg", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Image, plan.Kind);
        Assert.Equal(Path.GetFullPath(path), plan.Payload);
    }

    [Fact]
    public void PlanContentCopy_OtherFile_FallsBackToFileWithNote()
    {
        var path = WriteBytes("archive.zip", new byte[] { 0x50, 0x4B, 0x03, 0x04 });
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "archive.zip", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.True(plan.IsSuccess);
        // 其余格式回退复制文件本身，并给出状态栏补充说明（界面会展示 Note）。
        Assert.Equal(TransferCopyKind.File, plan.Kind);
        Assert.Equal(Path.GetFullPath(path), plan.Payload);
        Assert.Contains("不支持", plan.Note);
    }

    [Fact]
    public void PlanContentCopy_TextRecord_FailsBecauseSingleFileOnly()
    {
        // Type=text 的记录（如接收到的 .txt）不属于「单文件记录」，不提供内容复制语义。
        var path = WriteText("hello.txt", "hi", new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeText, Name = "hello.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("仅文件", plan.Error);
    }

    [Fact]
    public void PlanContentCopy_FileMissing_FailsWithChineseReason()
    {
        var record = new TransferRecord
        {
            Type = TransferConst.TypeFile,
            Name = "gone.bin",
            LocalPath = Path.Combine(_dir, "gone.bin")
        };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("不存在", plan.Error);
    }

    [Fact]
    public void PlanContentCopy_NoLocalPath_FailsWithChineseReason()
    {
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "无路径", LocalPath = "" };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("本地文件路径", plan.Error);
    }

    [Fact]
    public void PlanContentCopy_DirectoryPath_FailsWithFolderReason()
    {
        var folder = Path.Combine(_dir, "dir");
        Directory.CreateDirectory(folder);
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "dir", LocalPath = folder };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("文件夹", plan.Error);
    }

    [Fact]
    public void PlanContentCopy_TextFileOverLimit_FailsWithoutReading()
    {
        var path = WriteText("huge.txt", new string('y', 200), new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "huge.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record, maxTextBytes: 64);

        Assert.False(plan.IsSuccess);
        Assert.Contains("过大", plan.Error);
    }

    [Fact]
    public void PlanContentCopy_ImageOverLimit_FailsWithoutDecoding()
    {
        var path = WriteSizedFile("huge.png", TransferCopyPlanner.MaxImageBytes + 1);
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "huge.png", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.False(plan.IsSuccess);
        Assert.Contains("过大", plan.Error);
    }

    [Fact]
    public void PlanContentCopy_EmptyTextFile_ReturnsEmptyText()
    {
        var path = WriteText("empty.txt", "", new UTF8Encoding(false));
        var record = new TransferRecord { Type = TransferConst.TypeFile, Name = "empty.txt", LocalPath = path };

        var plan = TransferCopyPlanner.PlanContentCopy(record);

        Assert.True(plan.IsSuccess);
        Assert.Equal(TransferCopyKind.Text, plan.Kind);
        Assert.Equal("", plan.Payload);
    }

    // ---------------------------------------------------------------- 记录展示属性

    [Fact]
    public void IsFileRecord_TrueOnlyForFileType()
    {
        Assert.True(new TransferRecord { Type = TransferConst.TypeFile }.IsFileRecord);
        Assert.False(new TransferRecord { Type = TransferConst.TypeText }.IsFileRecord);
        Assert.False(new TransferRecord { Type = TransferConst.TypeFolder }.IsFileRecord);
    }
}
