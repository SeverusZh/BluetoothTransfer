using System.IO;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// ReceiveLocalPathResolver 单元测试：纯临时目录 + 纯文件系统逻辑（不依赖 WPF/存储/蓝牙），
/// 模拟 AsstFileSink「无冲突用 base 名、重名改为 stem (1).ext…（recv.bin → recv (1).bin）」的落盘结果，
/// 覆盖按文件大小区分重名旧文件、匹配不到时返回空串等关键路径。
/// </summary>
public class ReceiveLocalPathResolverTests : IDisposable
{
    private readonly string _dir;

    public ReceiveLocalPathResolverTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), "bt_recvpath_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(_dir);
    }

    public void Dispose()
    {
        try { Directory.Delete(_dir, recursive: true); } catch { }
    }

    private string WriteSizedFile(string name, long length)
    {
        // 用 SetLength 生成指定大小的文件：内容无关，只需要文件大小参与解析。
        var path = Path.Combine(_dir, name);
        using var fs = new FileStream(path, FileMode.CreateNew, FileAccess.Write);
        fs.SetLength(length);
        return path;
    }

    [Fact]
    public void Resolve_NoCollision_ReturnsBasePath()
    {
        var expected = WriteSizedFile("recv.bin", 123);

        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "recv.bin", 123);

        // 无重名时 AsstFileSink 直接落到 base 名，解析结果应为该文件的绝对路径。
        Assert.Equal(Path.GetFullPath(expected), resolved);
    }

    [Fact]
    public void Resolve_Collision_ReturnsSuffixedPath()
    {
        // 目录中已有同名旧文件（大小不同），本次落盘被 AsstFileSink 改名为 "recv (1).bin"。
        WriteSizedFile("recv.bin", 10);
        var expected = WriteSizedFile("recv (1).bin", 4000);

        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "recv.bin", 4000);

        Assert.Equal(Path.GetFullPath(expected), resolved);
    }

    [Fact]
    public void Resolve_MultipleSuffixes_PicksFirstSizeMatch()
    {
        WriteSizedFile("data.bt", 10);
        WriteSizedFile("data (1).bt", 20);
        var expected = WriteSizedFile("data (2).bt", 30);

        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "data.bt", 30);

        Assert.Equal(Path.GetFullPath(expected), resolved);
    }

    [Fact]
    public void Resolve_NoSizeMatch_ReturnsEmpty()
    {
        WriteSizedFile("recv.bin", 10);
        WriteSizedFile("recv (1).bin", 20);

        // 候选文件都存在但大小都对不上：宁可返回空串（复制规划器提示无本地路径），也不复制错文件。
        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "recv.bin", 999);

        Assert.Equal("", resolved);
    }

    [Fact]
    public void Resolve_FileMissing_ReturnsEmpty()
    {
        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "missing.bin", 123);

        Assert.Equal("", resolved);
    }

    [Fact]
    public void Resolve_EmptySaveDir_ReturnsEmpty()
    {
        Assert.Equal("", ReceiveLocalPathResolver.Resolve("", "recv.bin", 123));
        Assert.Equal("", ReceiveLocalPathResolver.Resolve("   ", "recv.bin", 123));
    }

    [Fact]
    public void Resolve_ZeroSizeFile_MatchesEmptyFile()
    {
        var expected = WriteSizedFile("empty.txt", 0);

        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "empty.txt", 0);

        Assert.Equal(Path.GetFullPath(expected), resolved);
    }

    [Fact]
    public void Resolve_ExtensionlessName_MatchesBaseName()
    {
        // 无扩展名文件无冲突时直接落到 base 名（AsstFileSink 的 ResolveFinalName 只在冲突时才加后缀）。
        var expected = WriteSizedFile("README", 7);

        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "README", 7);

        Assert.Equal(Path.GetFullPath(expected), resolved);
    }

    [Fact]
    public void Resolve_ExtensionlessCollision_ReturnsSuffixedName()
    {
        // 无扩展名文件冲突时 AsstFileSink 生成 "README (1)"（stem + 空格 + (i)，无扩展名后缀），解析需保持一致。
        WriteSizedFile("README", 10);
        var expected = WriteSizedFile("README (1)", 7);

        var resolved = ReceiveLocalPathResolver.Resolve(_dir, "README", 7);

        Assert.Equal(Path.GetFullPath(expected), resolved);
    }
}
