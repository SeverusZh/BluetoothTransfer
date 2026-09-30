using System.IO.Compression;
using System.Text;

namespace BluetoothTransfer.Services;

/// <summary>
/// 待续传打包产物存放区：zip（单文件/文件夹）与文本临时文件不再放 %TEMP% 且发送后立即删除，
/// 而是保留在 <c>%APPDATA%\BluetoothTransfer\pending</c>，保证传输完成前 SHA-256 可复现，
/// 从而重启后仍能命中接收端半成品续传。
/// 纯 BCL 实现（System.IO.Compression），跨平台可测。
/// </summary>
public sealed class PendingPackageStore : IPackageCleaner
{
    public const string DefaultDirectoryName = "pending";
    private const string Prefix = "bt_asst_pending_";

    public PendingPackageStore(string? directory = null)
    {
        PackageDirectory = string.IsNullOrWhiteSpace(directory)
            ? Path.Combine(FileTransferJournalStore.AppDataDirectory, DefaultDirectoryName)
            : Path.GetFullPath(directory);
    }

    /// <summary>产物目录（默认 %APPDATA%\BluetoothTransfer\pending）。</summary>
    public string PackageDirectory { get; }

    /// <summary>把单个文件打包为保留 zip，返回产物路径。</summary>
    public async Task<string> CreateZipFromFileAsync(string sourcePath, CancellationToken ct = default)
    {
        var entryName = new FileInfo(sourcePath).Name;
        var path = NewPath(".zip");
        await Task.Run(() =>
        {
            EnsureDirectory();
            using var archive = ZipFile.Open(path, ZipArchiveMode.Create);
            archive.CreateEntryFromFile(sourcePath, entryName, CompressionLevel.Optimal);
        }, ct);
        return path;
    }

    /// <summary>把文件夹打包为保留 zip（不含基目录），返回产物路径。</summary>
    public async Task<string> CreateZipFromFolderAsync(string folderPath, CancellationToken ct = default)
    {
        var path = NewPath(".zip");
        await Task.Run(() =>
        {
            EnsureDirectory();
            ZipFile.CreateFromDirectory(folderPath, path, CompressionLevel.Optimal, includeBaseDirectory: false);
        }, ct);
        return path;
    }

    /// <summary>把文本写成保留 .txt（UTF-8 无 BOM，与 OPP/助手通道既有口径一致），返回产物路径。</summary>
    public async Task<string> CreateTextFileAsync(string text, string fileName, CancellationToken ct = default)
    {
        var ext = Path.GetExtension(fileName);
        var path = NewPath(string.IsNullOrEmpty(ext) ? ".txt" : ext);
        EnsureDirectory();
        await File.WriteAllBytesAsync(path, Encoding.UTF8.GetBytes(text ?? ""), ct);
        return path;
    }

    /// <summary>路径是否位于本产物目录内（清理只允许删自己生成的产物，绝不误删用户文件）。</summary>
    public bool IsOwned(string? path)
    {
        if (string.IsNullOrWhiteSpace(path)) return false;
        string full;
        try { full = Path.GetFullPath(path); }
        catch { return false; }
        var root = Path.GetFullPath(PackageDirectory);
        var prefix = root.EndsWith(Path.DirectorySeparatorChar) ? root : root + Path.DirectorySeparatorChar;
        return full.StartsWith(prefix, StringComparison.OrdinalIgnoreCase);
    }

    /// <summary>删除本目录内的产物；目录外或不存在返回 false（安全兜底）。</summary>
    public bool Delete(string path)
    {
        if (!IsOwned(path)) return false;
        try
        {
            if (!File.Exists(path)) return false;
            File.Delete(path);
            return true;
        }
        catch
        {
            return false;
        }
    }

    /// <summary>清理未被任何 journal 条目引用且超过保留期的孤儿产物，返回清理个数。</summary>
    public int CleanOrphans(IEnumerable<string> referencedPaths, TimeSpan olderThan)
    {
        if (!Directory.Exists(PackageDirectory)) return 0;
        var referenced = new HashSet<string>(
            (referencedPaths ?? Enumerable.Empty<string>()).Where(p => !string.IsNullOrWhiteSpace(p)),
            StringComparer.OrdinalIgnoreCase);
        var cutoff = DateTime.UtcNow - olderThan;
        var count = 0;
        foreach (var file in Directory.EnumerateFiles(PackageDirectory))
        {
            try
            {
                if (referenced.Contains(Path.GetFullPath(file))) continue;
                if (File.GetLastWriteTimeUtc(file) >= cutoff) continue;
                File.Delete(file);
                count++;
            }
            catch
            {
                // 单个文件清理失败不影响其余
            }
        }
        return count;
    }

    private string NewPath(string extension)
    {
        EnsureDirectory();
        return Path.Combine(PackageDirectory, Prefix + Guid.NewGuid().ToString("N") + extension);
    }

    private void EnsureDirectory() => Directory.CreateDirectory(PackageDirectory);
}
