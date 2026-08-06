using System.Text.Json;
using System.Text.Json.Serialization;
using BluetoothTransfer.Core.Protocol;

namespace BluetoothTransfer.Core.Server;

/// <summary>MetaInfo 的源生成 JSON 上下文（裁剪模式下反射序列化不可用）。</summary>
[JsonSerializable(typeof(AsstFileSink.MetaInfo))]
internal sealed partial class AsstFileSinkJsonContext : JsonSerializerContext
{
}

/// <summary>
/// 磁盘落盘：接收中为 <文件名>.btpart + 同名 .btpart.meta（JSON 元数据），
/// 完成后改名为正式文件名（重名自动 name (1).ext）。
/// </summary>
public sealed class AsstFileSink : IAsstSink
{
    private readonly string _receiveDir;

    public AsstFileSink(string receiveDir)
    {
        _receiveDir = receiveDir ?? throw new ArgumentNullException(nameof(receiveDir));
        Directory.CreateDirectory(_receiveDir);
    }

    private string PartialPath(AsstHello hello) => Path.Combine(_receiveDir, Sanitize(hello.FileName) + ".btpart");
    private string MetaPath(AsstHello hello) => PartialPath(hello) + ".meta";

    public static string Sanitize(string name)
    {
        if (string.IsNullOrWhiteSpace(name)) return "file";
        var invalid = Path.GetInvalidFileNameChars();
        var chars = name.Select(c => invalid.Contains(c) ? '_' : c).ToArray();
        var result = new string(chars).Trim();
        return string.IsNullOrWhiteSpace(result) ? "file" : result;
    }

    public async Task<long> OpenAsync(AsstHello hello, CancellationToken ct = default)
    {
        var part = PartialPath(hello);
        var meta = MetaPath(hello);
        if (File.Exists(part) && File.Exists(meta))
        {
            try
            {
                var stored = JsonSerializer.Deserialize(await File.ReadAllTextAsync(meta, ct), AsstFileSinkJsonContext.Default.MetaInfo);
                if (stored != null && stored.TransferId == hello.TransferId && stored.FileSize == hello.FileSize)
                {
                    var len = new FileInfo(part).Length;
                    if (len <= hello.FileSize) return len;
                }
            }
            catch
            {
                // 元数据损坏视为不可续传
            }
        }
        TryDelete(part);
        TryDelete(meta);
        Directory.CreateDirectory(_receiveDir);
        await File.WriteAllTextAsync(meta, JsonSerializer.Serialize(new MetaInfo
        {
            TransferId = hello.TransferId,
            FileName = hello.FileName,
            FileSize = hello.FileSize,
            CreatedAt = DateTime.Now.ToString("o")
        }, AsstFileSinkJsonContext.Default.MetaInfo), ct);
        return 0;
    }

    public async Task WriteAsync(AsstHello hello, long offset, ReadOnlyMemory<byte> data, CancellationToken ct = default)
    {
        var part = PartialPath(hello);
        using var fs = new FileStream(part, FileMode.OpenOrCreate, FileAccess.Write, FileShare.None);
        if (fs.Length != offset)
            throw new AsstProtocolException($"偏移不一致：期望 {fs.Length}，收到 {offset}");
        fs.Seek(0, SeekOrigin.End);
        await fs.WriteAsync(data, ct);
        await fs.FlushAsync(ct);
        fs.Flush(flushToDisk: true); // ACK 前确保落盘，保证续传检查点可靠
    }

    public async Task<AsstDone> CompleteAsync(AsstHello hello, CancellationToken ct = default)
    {
        var part = PartialPath(hello);
        if (!File.Exists(part))
        {
            if (hello.FileSize == 0)
                await File.WriteAllBytesAsync(part, Array.Empty<byte>(), ct);
            else
                throw new AsstProtocolException("半成品文件不存在");
        }
        var actual = await AsstHash.ComputeSha256Async(part, ct);
        if (string.IsNullOrEmpty(hello.ExpectedSha256) ||
            string.Equals(actual, hello.ExpectedSha256, StringComparison.OrdinalIgnoreCase))
        {
            var final = ResolveFinalName(_receiveDir, hello.FileName);
            File.Move(part, final);
            TryDelete(MetaPath(hello));
            return new AsstDone(true, actual);
        }
        TryDelete(part);
        TryDelete(MetaPath(hello));
        return new AsstDone(false, actual);
    }

    public Task AbortAsync(AsstHello hello, bool deletePartial, CancellationToken ct = default)
    {
        if (deletePartial)
        {
            TryDelete(PartialPath(hello));
            TryDelete(MetaPath(hello));
        }
        return Task.CompletedTask;
    }

    /// <summary>冲突时自动生成 name (1).ext、name (2).ext …</summary>
    public static string ResolveFinalName(string dir, string fileName)
    {
        var clean = Sanitize(fileName);
        var candidate = Path.Combine(dir, clean);
        if (!File.Exists(candidate)) return candidate;
        var ext = Path.GetExtension(clean);
        var stem = Path.GetFileNameWithoutExtension(clean);
        for (var i = 1; ; i++)
        {
            candidate = Path.Combine(dir, $"{stem} ({i}){ext}");
            if (!File.Exists(candidate)) return candidate;
        }
    }

    /// <summary>清理超过 <paramref name="olderThan"/> 未更新的 .btpart/.btpart.meta 残留，返回清理个数。</summary>
    public static int CleanStale(string dir, TimeSpan olderThan)
    {
        if (!Directory.Exists(dir)) return 0;
        var count = 0;
        var cutoff = DateTime.Now - olderThan;
        foreach (var f in Directory.EnumerateFiles(dir, "*.btpart"))
        {
            try
            {
                if (File.GetLastWriteTime(f) < cutoff)
                {
                    File.Delete(f);
                    File.Delete(f + ".meta");
                    count++;
                }
            }
            catch
            {
                // 单个文件清理失败不影响其余
            }
        }
        return count;
    }

    private static void TryDelete(string path)
    {
        try { if (File.Exists(path)) File.Delete(path); } catch { }
    }

    internal sealed class MetaInfo
    {
        public string TransferId { get; set; } = "";
        public string FileName { get; set; } = "";
        public long FileSize { get; set; }
        public string CreatedAt { get; set; } = "";
    }
}
