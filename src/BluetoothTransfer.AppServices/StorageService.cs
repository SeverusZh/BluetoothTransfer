using System.IO;
using Microsoft.Data.Sqlite;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class StorageService : IStorageService
{
    private readonly string _dbPath;
    private readonly string _connStr;

    public StorageService() : this(null)
    {
    }

    /// <summary>
    /// <paramref name="dbPath"/> 为空时使用默认用户目录数据库（%APPDATA%\BluetoothTransfer\data.db）；
    /// 指定时使用给定路径（如 CLI 自检用临时库），避免污染真实记录。
    /// </summary>
    public StorageService(string? dbPath)
    {
        if (string.IsNullOrEmpty(dbPath))
        {
            var dir = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "BluetoothTransfer");
            Directory.CreateDirectory(dir);
            _dbPath = Path.Combine(dir, "data.db");
        }
        else
        {
            var fullPath = Path.GetFullPath(dbPath);
            Directory.CreateDirectory(Path.GetDirectoryName(fullPath) ?? ".");
            _dbPath = fullPath;
        }
        // 使用 SqliteConnectionStringBuilder 构造连接串，避免直接字符串拼接导致
        // 路径含 ';、" 等字符时被解析为非法连接串（或注入额外连接参数）。
        _connStr = new SqliteConnectionStringBuilder { DataSource = _dbPath }.ToString();
        InitDb();
    }

    private void InitDb()
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();

        // 从 PRAGMA user_version 读取当前库版本；整数类型，不涉及外部拼接，直接读取即可。
        long userVersion;
        using (var versionCmd = conn.CreateCommand())
        {
            versionCmd.CommandText = "PRAGMA user_version;";
            userVersion = (long)versionCmd.ExecuteScalar()!;
        }

        // 迁移到版本 1：版本 0（含全新车库/未跟踪版本的旧库）在此建立基础表结构，
        // 保持既有表结构不变，仅把 user_version 置为 1；未来 schema 演进在此追加
        // 更高版本的迁移分支。写入 user_version 用参数化防止拼接外部输入。
        if (userVersion < 1)
        {
            using var cmd = conn.CreateCommand();
            cmd.CommandText = """
                CREATE TABLE IF NOT EXISTS transfer_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    created_at TEXT NOT NULL,
                    direction TEXT NOT NULL,
                    type TEXT NOT NULL,
                    peer_name TEXT,
                    peer_addr TEXT,
                    name TEXT,
                    size INTEGER DEFAULT 0,
                    status TEXT DEFAULT 'ok',
                    checksum TEXT,
                    channel TEXT DEFAULT 'ble',
                    local_path TEXT,
                    note TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_records_created ON transfer_records(created_at);
                CREATE INDEX IF NOT EXISTS idx_records_peer ON transfer_records(peer_addr);
                CREATE INDEX IF NOT EXISTS idx_records_direction ON transfer_records(direction);
                CREATE INDEX IF NOT EXISTS idx_records_status ON transfer_records(status);
                CREATE INDEX IF NOT EXISTS idx_records_type ON transfer_records(type);

                CREATE TABLE IF NOT EXISTS devices (
                    addr TEXT PRIMARY KEY,
                    name TEXT,
                    alias TEXT,
                    favorite INTEGER DEFAULT 0,
                    last_seen TEXT,
                    last_connected TEXT
                );
                """;
            cmd.ExecuteNonQuery();

            // 写入 user_version：SQLite 的 PRAGMA 赋值不支持绑定参数（会报语法错误），
            // 这里使用编译期整数常量（无外部输入，安全）。
            using var setVersion = conn.CreateCommand();
            setVersion.CommandText = "PRAGMA user_version = 1;";
            setVersion.ExecuteNonQuery();
        }
    }

    public void AddRecord(TransferRecord record)
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            INSERT INTO transfer_records (created_at, direction, type, peer_name, peer_addr, name, size, status, checksum, channel, local_path, note)
            VALUES (@created_at, @direction, @type, @peer_name, @peer_addr, @name, @size, @status, @checksum, @channel, @local_path, @note)
            """;
        // created_at 一律落 UTC 时间（"o" 格式以 Z 结尾），保证 created_at 是字典序
        // 即时间序，字符串比较/排序逻辑才不会因时区偏移混入而失效。
        // 遗留说明：旧库中已存在的行仍可能是本地时间偏移格式（如 DateTime.Now 写入），
        // 本次加固不迁移转换这些旧行，仅对新增写入保证 UTC 格式。
        var createdUtc = DateTime.UtcNow.ToString("o");
        cmd.Parameters.AddWithValue("@created_at", string.IsNullOrEmpty(record.CreatedAt) ? createdUtc : record.CreatedAt);
        cmd.Parameters.AddWithValue("@direction", record.Direction);
        cmd.Parameters.AddWithValue("@type", record.Type);
        cmd.Parameters.AddWithValue("@peer_name", record.PeerName);
        cmd.Parameters.AddWithValue("@peer_addr", record.PeerAddr);
        cmd.Parameters.AddWithValue("@name", record.Name);
        cmd.Parameters.AddWithValue("@size", record.Size);
        cmd.Parameters.AddWithValue("@status", record.Status);
        cmd.Parameters.AddWithValue("@checksum", record.Checksum);
        cmd.Parameters.AddWithValue("@channel", record.Channel);
        cmd.Parameters.AddWithValue("@local_path", record.LocalPath);
        cmd.Parameters.AddWithValue("@note", record.Note);
        cmd.ExecuteNonQuery();
    }

    private static readonly HashSet<string> AllowedOrderBy = new(StringComparer.OrdinalIgnoreCase)
    {
        "created_at DESC", "created_at ASC", "size DESC", "size ASC",
        "status DESC", "status ASC", "name DESC", "name ASC"
    };

    public List<TransferRecord> GetRecords(string? direction = null, string? type = null,
        string? peerAddr = null, string? status = null, DateTime? from = null, DateTime? to = null,
        string? search = null, string orderBy = "created_at DESC", int limit = 200)
    {
        if (!AllowedOrderBy.Contains(orderBy))
            orderBy = "created_at DESC";

        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();

        var where = new List<string>();
        if (direction != null) { where.Add("direction = @direction"); cmd.Parameters.AddWithValue("@direction", direction); }
        if (type != null) { where.Add("type = @type"); cmd.Parameters.AddWithValue("@type", type); }
        if (peerAddr != null) { where.Add("peer_addr = @peer_addr"); cmd.Parameters.AddWithValue("@peer_addr", peerAddr); }
        if (status != null) { where.Add("status = @status"); cmd.Parameters.AddWithValue("@status", status); }
        // created_at 现按 UTC 存储（Z 结尾字典序即时间序），from/to 统一转 UTC 后
        // 再参与字符串比较，避免调用方传入本地偏移时间导致比较错位。
        if (from != null) { where.Add("created_at >= @from"); cmd.Parameters.AddWithValue("@from", from.Value.ToUniversalTime().ToString("o")); }
        if (to != null) { where.Add("created_at <= @to"); cmd.Parameters.AddWithValue("@to", to.Value.ToUniversalTime().ToString("o")); }
        if (search != null) { where.Add("(name LIKE @search OR peer_name LIKE @search)"); cmd.Parameters.AddWithValue("@search", $"%{search}%"); }

        var whereClause = where.Count > 0 ? "WHERE " + string.Join(" AND ", where) : "";
        cmd.CommandText = $"SELECT * FROM transfer_records {whereClause} ORDER BY {orderBy} LIMIT @limit";
        cmd.Parameters.AddWithValue("@limit", limit);

        var records = new List<TransferRecord>();
        using var reader = cmd.ExecuteReader();
        while (reader.Read())
        {
            records.Add(new TransferRecord
            {
                Id = reader.GetInt64(0),
                CreatedAt = reader.GetString(1),
                Direction = reader.GetString(2),
                Type = reader.GetString(3),
                PeerName = reader.IsDBNull(4) ? "" : reader.GetString(4),
                PeerAddr = reader.IsDBNull(5) ? "" : reader.GetString(5),
                Name = reader.IsDBNull(6) ? "" : reader.GetString(6),
                Size = reader.GetInt64(7),
                Status = reader.GetString(8),
                Checksum = reader.IsDBNull(9) ? "" : reader.GetString(9),
                Channel = reader.IsDBNull(10) ? "ble" : reader.GetString(10),
                LocalPath = reader.IsDBNull(11) ? "" : reader.GetString(11),
                Note = reader.IsDBNull(12) ? "" : reader.GetString(12)
            });
        }
        return records;
    }

    public void DeleteRecord(long id)
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "DELETE FROM transfer_records WHERE id = @id";
        cmd.Parameters.AddWithValue("@id", id);
        cmd.ExecuteNonQuery();
    }

    public void CleanRecordsBefore(DateTime before)
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "DELETE FROM transfer_records WHERE created_at < @before";
        // 与 GetRecords 一致：按 UTC 存储语义转成 UTC 后比较。
        cmd.Parameters.AddWithValue("@before", before.ToUniversalTime().ToString("o"));
        cmd.ExecuteNonQuery();
    }

    /// <summary>清空全部传输记录（保留设备表与配置），返回删除条数。</summary>
    public int ClearRecords()
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "DELETE FROM transfer_records";
        return cmd.ExecuteNonQuery();
    }

    public void UpsertDevice(DeviceInfo device)
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            INSERT INTO devices (addr, name, alias, favorite, last_seen, last_connected)
            VALUES (@addr, @name, @alias, @favorite, @last_seen, @last_connected)
            ON CONFLICT(addr) DO UPDATE SET
                name = @name,
                alias = CASE WHEN @alias != '' THEN @alias ELSE alias END,
                favorite = CASE WHEN @favorite != 0 THEN @favorite ELSE favorite END,
                last_seen = @last_seen,
                last_connected = CASE WHEN @last_connected != '' THEN @last_connected ELSE last_connected END
            """;
        cmd.Parameters.AddWithValue("@addr", device.Addr);
        cmd.Parameters.AddWithValue("@name", device.Name);
        cmd.Parameters.AddWithValue("@alias", device.Alias);
        cmd.Parameters.AddWithValue("@favorite", device.Favorite ? 1 : 0);
        cmd.Parameters.AddWithValue("@last_seen", device.LastSeen);
        cmd.Parameters.AddWithValue("@last_connected", device.LastConnected);
        cmd.ExecuteNonQuery();
    }

    /// <summary>设置设备收藏状态，返回是否存在该设备。</summary>
    public bool SetDeviceFavorite(string addr, bool favorite)
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "UPDATE devices SET favorite = @favorite WHERE addr = @addr";
        cmd.Parameters.AddWithValue("@favorite", favorite ? 1 : 0);
        cmd.Parameters.AddWithValue("@addr", addr);
        return cmd.ExecuteNonQuery() > 0;
    }

    /// <summary>设置设备别名（空字符串清除别名），返回是否存在该设备。</summary>
    public bool SetDeviceAlias(string addr, string alias)
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "UPDATE devices SET alias = @alias WHERE addr = @addr";
        cmd.Parameters.AddWithValue("@alias", alias ?? "");
        cmd.Parameters.AddWithValue("@addr", addr);
        return cmd.ExecuteNonQuery() > 0;
    }

    public List<DeviceInfo> GetDevices()
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "SELECT * FROM devices ORDER BY favorite DESC, last_connected DESC";

        var devices = new List<DeviceInfo>();
        using var reader = cmd.ExecuteReader();
        while (reader.Read())
        {
            devices.Add(new DeviceInfo
            {
                Addr = reader.GetString(0),
                Name = reader.IsDBNull(1) ? "" : reader.GetString(1),
                Alias = reader.IsDBNull(2) ? "" : reader.GetString(2),
                Favorite = reader.GetInt32(3) == 1,
                LastSeen = reader.IsDBNull(4) ? "" : reader.GetString(4),
                LastConnected = reader.IsDBNull(5) ? "" : reader.GetString(5)
            });
        }
        return devices;
    }

    public (long totalCount, long totalBytes, long sendCount, long recvCount) GetStats()
    {
        using var conn = new SqliteConnection(_connStr);
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            SELECT
                COUNT(*),
                COALESCE(SUM(size), 0),
                COALESCE(SUM(CASE WHEN direction=@dir_send THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN direction=@dir_recv THEN 1 ELSE 0 END), 0)
            FROM transfer_records WHERE status = 'ok'
            """;
        cmd.Parameters.AddWithValue("@dir_send", TransferConst.DirSend);
        cmd.Parameters.AddWithValue("@dir_recv", TransferConst.DirRecv);
        using var reader = cmd.ExecuteReader();
        reader.Read();
        return (reader.GetInt64(0), reader.GetInt64(1), reader.GetInt64(2), reader.GetInt64(3));
    }
}
