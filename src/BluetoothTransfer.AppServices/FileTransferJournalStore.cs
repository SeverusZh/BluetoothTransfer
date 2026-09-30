using System.Text.Json;
using System.Text.Json.Serialization;

namespace BluetoothTransfer.Services;

/// <summary>journal JSON 的源生成上下文（与 AsstFileSink 一致，避免裁剪模式下反射序列化不可用）。</summary>
[JsonSerializable(typeof(List<TransferJournalEntry>))]
internal sealed partial class TransferJournalJsonContext : JsonSerializerContext
{
}

/// <summary>
/// 发送日志的原子 JSON 文件实现。
/// 选型理由（相对 SQLite/StorageService）：
/// 1) 纯 BCL、无原生依赖，跨平台可测（Linux 上也能跑真实回归测试）；
/// 2) 恢复需在 UI 启动前完成，journal 规模是个位数~几十条，一次小文件读远比开库+user_version 迁移轻；
/// 3) 不把发送生命周期耦合进 records/devices 库（GUI/CLI/Receiver 各自用不同 db 路径，共享语义不清）。
/// 原子性：写 &lt;file&gt;.tmp 后 File.Move(overwrite) 替换；读-改-写全程持有 &lt;file&gt;.lock 跨进程独占锁，
/// 因此 &quot;认领/续租/释放/删除&quot; 之间不会丢更新。
/// </summary>
public sealed class FileTransferJournalStore : ITransferJournalStore
{
    public const string DefaultFileName = "send-journal.json";

    private static readonly object ProcessLockTable = new();
    private static readonly Dictionary<string, object> ProcessLocks = new(StringComparer.OrdinalIgnoreCase);

    private readonly string _path;
    private readonly string _lockPath;
    private readonly object _processLock;

    public FileTransferJournalStore(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) throw new ArgumentException("journal 路径不能为空", nameof(path));
        _path = Path.GetFullPath(path);
        _lockPath = _path + ".lock";
        lock (ProcessLockTable)
        {
            if (!ProcessLocks.TryGetValue(_path, out var gate))
            {
                gate = new object();
                ProcessLocks[_path] = gate;
            }
            _processLock = gate;
        }
    }

    /// <summary>默认 journal 路径：%APPDATA%\BluetoothTransfer\send-journal.json。</summary>
    public static string DefaultPath => Path.Combine(AppDataDirectory, DefaultFileName);

    /// <summary>应用数据目录（与 StorageService 默认库同目录）。</summary>
    public static string AppDataDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "BluetoothTransfer");

    public string JournalPath => _path;

    public IReadOnlyList<TransferJournalEntry> ReadAll()
    {
        lock (_processLock)
        {
            using var fileLock = AcquireFileLock();
            return ReadUnlocked();
        }
    }

    public T Mutate<T>(Func<List<TransferJournalEntry>, T> mutate)
    {
        ArgumentNullException.ThrowIfNull(mutate);
        lock (_processLock)
        {
            using var fileLock = AcquireFileLock();
            var entries = ReadUnlocked();
            var result = mutate(entries);
            WriteAtomic(entries);
            return result;
        }
    }

    private List<TransferJournalEntry> ReadUnlocked()
    {
        if (!File.Exists(_path)) return new List<TransferJournalEntry>();
        try
        {
            var json = File.ReadAllText(_path);
            if (string.IsNullOrWhiteSpace(json)) return new List<TransferJournalEntry>();
            return JsonSerializer.Deserialize(json, TransferJournalJsonContext.Default.ListTransferJournalEntry)
                   ?? new List<TransferJournalEntry>();
        }
        catch (JsonException)
        {
            // 文件损坏（断电写入等）：改名留证并当作空 journal，避免恢复流程整体失败。
            TryMoveAside();
            return new List<TransferJournalEntry>();
        }
        catch (IOException)
        {
            return new List<TransferJournalEntry>();
        }
    }

    private void WriteAtomic(List<TransferJournalEntry> entries)
    {
        var dir = Path.GetDirectoryName(_path);
        if (!string.IsNullOrEmpty(dir)) Directory.CreateDirectory(dir);
        var tmp = _path + ".tmp";
        try
        {
            var json = JsonSerializer.Serialize(entries, TransferJournalJsonContext.Default.ListTransferJournalEntry);
            File.WriteAllText(tmp, json);
            File.Move(tmp, _path, overwrite: true);
        }
        catch
        {
            TryDelete(tmp);
            throw;
        }
    }

    /// <summary>跨进程独占锁（按路径固定一个 .lock 文件）；同进程内由 _processLock 先串行化。</summary>
    private IDisposable AcquireFileLock()
    {
        var dir = Path.GetDirectoryName(_path);
        if (!string.IsNullOrEmpty(dir)) Directory.CreateDirectory(dir);
        var deadline = DateTime.UtcNow + TimeSpan.FromSeconds(5);
        while (true)
        {
            try
            {
                return new FileStream(_lockPath, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None);
            }
            catch (IOException) when (DateTime.UtcNow < deadline)
            {
                Thread.Sleep(20);
            }
            catch (UnauthorizedAccessException) when (DateTime.UtcNow < deadline)
            {
                Thread.Sleep(20);
            }
        }
    }

    private void TryMoveAside()
    {
        try
        {
            if (File.Exists(_path)) File.Move(_path, _path + ".corrupt", overwrite: true);
        }
        catch
        {
            // 留证失败不影响流程
        }
    }

    private static void TryDelete(string path)
    {
        try { if (File.Exists(path)) File.Delete(path); } catch { }
    }
}
