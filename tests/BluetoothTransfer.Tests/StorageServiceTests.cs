using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class StorageServiceTests : IDisposable
{
    private readonly string _dbPath;

    public StorageServiceTests()
    {
        _dbPath = Path.Combine(Path.GetTempPath(), $"bt_test_{Guid.NewGuid():N}.db");
    }

    public void Dispose()
    {
        try
        {
            if (File.Exists(_dbPath)) File.Delete(_dbPath);
        }
        catch
        {
            // 清理失败不影响断言
        }
    }

    private static TransferRecord SampleRecord(string direction = TransferConst.DirSend, string status = TransferConst.StatusOk)
        => new()
        {
            Direction = direction,
            Type = TransferConst.TypeFile,
            PeerName = "测试设备",
            PeerAddr = "00:11:22:33:44:55",
            Name = "test.bin",
            Size = 1024,
            Status = status,
            Channel = TransferConst.ChannelOpp
        };

    [Fact]
    public void ClearRecords_DeletesAllRecords_KeepsDevices()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord(TransferConst.DirRecv, TransferConst.StatusFailed));
        storage.UpsertDevice(new DeviceInfo { Addr = "00:11:22:33:44:55", Name = "测试设备" });

        Assert.Equal(2, storage.GetRecords().Count);
        Assert.Single(storage.GetDevices());

        var deleted = storage.ClearRecords();

        Assert.Equal(2, deleted);
        Assert.Empty(storage.GetRecords());
        Assert.Single(storage.GetDevices()); // 设备表不受影响
    }

    [Fact]
    public void ClearRecords_StatsResetToZero()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord());

        var before = storage.GetStats();
        Assert.Equal(2, before.totalCount);
        Assert.Equal(2, before.sendCount);

        storage.ClearRecords();

        var after = storage.GetStats();
        Assert.Equal(0, after.totalCount);
        Assert.Equal(0, after.totalBytes);
        Assert.Equal(0, after.sendCount);
        Assert.Equal(0, after.recvCount);
    }

    [Fact]
    public void ClearRecords_OnEmptyTable_ReturnsZero()
    {
        var storage = new StorageService(_dbPath);
        Assert.Equal(0, storage.ClearRecords());
    }

    [Fact]
    public void DeviceFavoriteAndAlias_RoundTrip()
    {
        var storage = new StorageService(_dbPath);
        storage.UpsertDevice(new DeviceInfo { Addr = "00:11:22:33:44:55", Name = "手机" });

        Assert.True(storage.SetDeviceFavorite("00:11:22:33:44:55", true));
        Assert.True(storage.SetDeviceAlias("00:11:22:33:44:55", "我的小米"));

        var device = storage.GetDevices().Single();
        Assert.True(device.Favorite);
        Assert.Equal("我的小米", device.Alias);

        Assert.True(storage.SetDeviceAlias("00:11:22:33:44:55", ""));
        Assert.Equal("", storage.GetDevices().Single().Alias);
        Assert.True(storage.SetDeviceFavorite("00:11:22:33:44:55", false));
        Assert.False(storage.GetDevices().Single().Favorite);
    }

    [Fact]
    public void DeviceFavorite_UnknownDevice_ReturnsFalse()
    {
        var storage = new StorageService(_dbPath);
        Assert.False(storage.SetDeviceFavorite("00:00:00:00:00:00", true));
        Assert.False(storage.SetDeviceAlias("00:00:00:00:00:00", "x"));
    }

    // ------ 加固：连接串 / UTC / user_version 迁移 / 查询 ------ //

    [Theory]
    [InlineData("db;name.db")]
    [InlineData("db=with=equals.db")]
    [InlineData("db ' spaced'.db")]
    [InlineData("带分号;的名称.db")]
    public void ConnectionStringBuilder_HandlesSpecialCharactersInPath(string fileName)
    {
        // 路径含 ';、=、文字等字符时仍能正常建库并读写，说明连接串构造不再用简单拼接。
        // （双引号虽在解析中最危险，但 Windows 不允许文件名含 "，故此处仅覆盖可落盘的字符。）
        var special = Path.Combine(Path.GetTempPath(), fileName);
        try
        {
            var storage = new StorageService(special);
            storage.AddRecord(SampleRecord());
            Assert.Single(storage.GetRecords());
        }
        finally
        {
            // 连接池可能仍持有 db 文件句柄，先清空池再删除；清理失败不掩盖断言结果。
            try
            {
                Microsoft.Data.Sqlite.SqliteConnection.ClearAllPools();
                if (File.Exists(special)) File.Delete(special);
            }
            catch
            {
                // 清理失败不影响断言
            }
        }
    }

    [Fact]
    public void AddRecord_WritesUtcTimestamp_EndsWithZ()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());

        var record = storage.GetRecords().Single();
        // "o" 格式的 UTC 时间以 Z 结尾（如 2024-01-01T00:00:00.0000000Z）。
        Assert.EndsWith("Z", record.CreatedAt);
        // 与（近似）当前 UTC 时间对比，确保不是本地偏移格式导致顺序错乱。
        var parsed = DateTimeOffset.Parse(record.CreatedAt, null,
            System.Globalization.DateTimeStyles.RoundtripKind);
        Assert.Equal(DateTimeOffset.UtcNow.Offset, parsed.Offset);
        Assert.True(DateTimeOffset.UtcNow - parsed < TimeSpan.FromSeconds(10));
    }

    /// <summary>读取指定库文件的 user_version。</summary>
    private static long ReadUserVersion(string dbPath)
    {
        using var conn = new Microsoft.Data.Sqlite.SqliteConnection($"Data Source={dbPath}");
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = "PRAGMA user_version;";
        return (long)cmd.ExecuteScalar()!;
    }

    /// <summary>v1 基础表结构（第二阶段迁移前），用于模拟旧库。</summary>
    private const string V1Schema = """
        CREATE TABLE transfer_records (
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
        CREATE TABLE devices (
            addr TEXT PRIMARY KEY,
            name TEXT,
            alias TEXT,
            favorite INTEGER DEFAULT 0,
            last_seen TEXT,
            last_connected TEXT
        );
        """;

    /// <summary>手工建一个 v1 旧库（可指定 user_version 与是否已提前加过 content 列）。</summary>
    private void CreateV1Database(long? userVersion = null, bool withContentColumn = false)
    {
        using var conn = new Microsoft.Data.Sqlite.SqliteConnection(
            new Microsoft.Data.Sqlite.SqliteConnectionStringBuilder { DataSource = _dbPath }.ToString());
        conn.Open();
        using (var cmd = conn.CreateCommand())
        {
            cmd.CommandText = withContentColumn
                ? V1Schema + "\nALTER TABLE transfer_records ADD COLUMN content TEXT;"
                : V1Schema;
            cmd.ExecuteNonQuery();
        }
        if (userVersion.HasValue)
        {
            using var version = conn.CreateCommand();
            // 测试内固定整数常量，无外部输入。
            version.CommandText = $"PRAGMA user_version = {userVersion.Value};";
            version.ExecuteNonQuery();
        }
    }

    /// <summary>向当前库插入一条 v1 格式的旧记录（不带 content 列）。</summary>
    private void InsertLegacyRecord(string name = "旧记录.txt", string type = TransferConst.TypeText)
    {
        using var conn = new Microsoft.Data.Sqlite.SqliteConnection($"Data Source={_dbPath}");
        conn.Open();
        using var cmd = conn.CreateCommand();
        cmd.CommandText = """
            INSERT INTO transfer_records (created_at, direction, type, peer_name, peer_addr, name, size, status, channel, local_path, note)
            VALUES ('2024-01-01T00:00:00.0000000Z', 'send', @type, '旧设备', '00:11:22:33:44:55', @name, 12, 'ok', 'opp', '', '旧版记录')
            """;
        cmd.Parameters.AddWithValue("@type", type);
        cmd.Parameters.AddWithValue("@name", name);
        cmd.ExecuteNonQuery();
    }

    [Fact]
    public void InitDb_NewDatabase_MigratesToLatestVersionAndCreatesTables()
    {
        var storage = new StorageService(_dbPath);
        Assert.Equal(2L, ReadUserVersion(_dbPath));
        Assert.Empty(storage.GetRecords()); // 表可用
        Assert.Empty(storage.GetDevices());
    }

    [Fact]
    public void InitDb_OldDatabaseWithoutUserVersion_MigratesSequentiallyToLatestVersion()
    {
        // 手工建一张没有 user_version 的旧库（复制迁移前的表结构），InitDb 应按 0→1→2 顺序升级，
        // 并在不破坏已有表的前提下补齐索引/设备表/content 列。
        CreateV1Database(userVersion: null);
        Assert.Equal(0L, ReadUserVersion(_dbPath));

        var storage = new StorageService(_dbPath);

        Assert.Equal(2L, ReadUserVersion(_dbPath));
        Assert.Empty(storage.GetRecords());
        Assert.Empty(storage.GetDevices());
    }

    [Fact]
    public void InitDb_VersionOneDatabase_MigratesToTwoAndPreservesLegacyRows()
    {
        // 第二阶段（v1）真实用户库：已有记录但没有 content 列。升级后旧记录仍可读，
        // 新增字段为 NULL → Content = ""（复制时回退 LocalPath 文件）。
        CreateV1Database(userVersion: 1);
        InsertLegacyRecord();

        var storage = new StorageService(_dbPath);

        Assert.Equal(2L, ReadUserVersion(_dbPath));
        var legacy = Assert.Single(storage.GetRecords());
        Assert.Equal("旧记录.txt", legacy.Name);
        Assert.Equal(TransferConst.TypeText, legacy.Type);
        Assert.Equal("", legacy.Content);
        Assert.False(legacy.HasContent);
        Assert.Equal("旧版记录", legacy.Note);
    }

    [Fact]
    public void InitDb_ReopenMigratedDatabase_IsIdempotentAndKeepsContent()
    {
        var first = new StorageService(_dbPath);
        first.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeText,
            Name = "note.txt",
            Size = 6,
            Status = TransferConst.StatusOk,
            Channel = TransferConst.ChannelOpp,
            Content = "第一段正文"
        });

        // 重复打开（模拟应用多次启动）不应重复执行 ALTER / 报错，旧数据与正文保持不变。
        _ = new StorageService(_dbPath);
        var third = new StorageService(_dbPath);

        Assert.Equal(2L, ReadUserVersion(_dbPath));
        var record = Assert.Single(third.GetRecords());
        Assert.Equal("第一段正文", record.Content);
        Assert.True(record.HasContent);
    }

    [Fact]
    public void InitDb_VersionOneButContentColumnAlreadyPresent_CompletesWithoutDuplicateColumnError()
    {
        // 防御性覆盖：假设上次迁移已完成 ALTER 但 user_version 落盘前被强杀，
        // 再次打开库时不得因 duplicate column 崩溃，且应把版本补齐到 2。
        CreateV1Database(userVersion: 1, withContentColumn: true);
        InsertLegacyRecord();

        var storage = new StorageService(_dbPath);

        Assert.Equal(2L, ReadUserVersion(_dbPath));
        Assert.Single(storage.GetRecords());
    }

    [Fact]
    public void AddRecord_ContentRoundTrips_AndIsEmptyForRecordsWithoutContent()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeText,
            Name = "你好.txt",
            Size = 18,
            Status = TransferConst.StatusOk,
            Channel = TransferConst.ChannelAssistant,
            Content = "你好，蓝牙 😀\n第二行"
        });
        storage.AddRecord(SampleRecord());

        var records = storage.GetRecords();
        Assert.Equal(2, records.Count);
        var text = records.Single(r => r.Type == TransferConst.TypeText);
        Assert.Equal("你好，蓝牙 😀\n第二行", text.Content);
        Assert.True(text.HasContent);
        var file = records.Single(r => r.Type == TransferConst.TypeFile);
        Assert.Equal("", file.Content);
        Assert.False(file.HasContent);
    }

    [Fact]
    public void GetRecords_TypeFolderFilter_Works()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeFolder,
            Name = "docs.zip",
            Size = 1024,
            Status = TransferConst.StatusOk,
            Channel = TransferConst.ChannelOpp,
            LocalPath = Path.Combine(Path.GetTempPath(), "docs")
        });
        storage.AddRecord(SampleRecord());

        var folders = storage.GetRecords(type: TransferConst.TypeFolder);

        var folder = Assert.Single(folders);
        Assert.Equal(TransferConst.TypeFolder, folder.Type);
        Assert.Equal("文件夹", folder.TypeDisplay);
        Assert.Single(storage.GetRecords(type: TransferConst.TypeFile));
    }

    [Fact]
    public void Export_DoesNotContainContent_LegacyFormatUnchanged()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeText,
            Name = "secret.txt",
            Size = 20,
            Status = TransferConst.StatusOk,
            Channel = TransferConst.ChannelOpp,
            Content = "绝密正文ABC-不应出现在导出中"
        });
        var records = storage.GetRecords();

        var jsonPath = Path.Combine(Path.GetTempPath(), $"bt_export_{Guid.NewGuid():N}.json");
        var csvPath = Path.Combine(Path.GetTempPath(), $"bt_export_{Guid.NewGuid():N}.csv");
        try
        {
            var json = File.ReadAllText(ExportService.ExportJson(records, jsonPath));
            var csv = File.ReadAllText(ExportService.ExportCsv(records, csvPath));

            // JsonIgnore：正文与 HasContent 都不进入 JSON 导出（保持 v1 格式与体积）。
            Assert.DoesNotContain("绝密正文ABC", json);
            Assert.DoesNotContain("\"Content\"", json);
            Assert.DoesNotContain("\"HasContent\"", json);
            // JSON 仍按原字段集导出。
            Assert.Contains("\"Name\": \"secret.txt\"", json);
            // CSV 显式列举列，本身就不含 content。
            Assert.DoesNotContain("绝密正文ABC", csv);
            Assert.StartsWith("Id,CreatedAt,Direction,Type,PeerName,PeerAddr,Name,Size,Status,Checksum,Channel,LocalPath,Note", csv);
        }
        finally
        {
            try
            {
                if (File.Exists(jsonPath)) File.Delete(jsonPath);
                if (File.Exists(csvPath)) File.Delete(csvPath);
            }
            catch
            {
                // 清理失败不影响断言
            }
        }
    }

    [Fact]
    public void GetRecords_DirectionTypeStatusFilters()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord(TransferConst.DirSend, TransferConst.StatusOk));
        storage.AddRecord(SampleRecord(TransferConst.DirRecv, TransferConst.StatusOk));
        storage.AddRecord(SampleRecord(TransferConst.DirSend, TransferConst.StatusFailed));

        Assert.Single(storage.GetRecords(direction: TransferConst.DirRecv));
        Assert.Single(storage.GetRecords(direction: TransferConst.DirSend, status: TransferConst.StatusFailed));
        Assert.Equal(2, storage.GetRecords(direction: TransferConst.DirSend).Count);
        Assert.Equal(2, storage.GetRecords(status: TransferConst.StatusOk).Count);
    }

    [Fact]
    public void GetRecords_SearchFiltersNameOrPeerName()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        var other = SampleRecord();
        other.Name = "report.pdf";
        other.PeerName = "另一方";
        storage.AddRecord(other);

        Assert.Single(storage.GetRecords(search: "report"));
        Assert.Single(storage.GetRecords(search: "另一方"));
        Assert.Single(storage.GetRecords(search: "test")); // 命中第一个的 Name=test.bin
    }

    [Fact]
    public void GetRecords_TypeFilterAndLimit()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord(TransferConst.DirRecv));

        // 现有 SampleRecord 均 type=file 且 peer_addr 相同；用 type 过滤两次都在。
        Assert.Equal(2, storage.GetRecords(type: TransferConst.TypeFile).Count);
        // limit 截断。
        Assert.Single(storage.GetRecords(limit: 1));
    }

    [Fact]
    public void DeleteRecord_RemovesOne()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord());
        Assert.Equal(2, storage.GetRecords().Count);

        var id = storage.GetRecords().First().Id;
        storage.DeleteRecord(id);

        var records = storage.GetRecords();
        Assert.Single(records);
        Assert.DoesNotContain(records, r => r.Id == id);
    }

    [Fact]
    public void CleanRecordsBefore_RemovesOlderThanBoundary()
    {
        var storage = new StorageService(_dbPath);
        storage.AddRecord(SampleRecord());
        storage.AddRecord(SampleRecord());
        Assert.Equal(2, storage.GetRecords().Count);

        // 记录为刚写入的 UTC 时间，删除"很久以前"之前的数据不会删掉它们。
        storage.CleanRecordsBefore(DateTime.UtcNow.AddDays(-1));
        Assert.Equal(2, storage.GetRecords().Count);

        // 删除"未来时刻"之前的数据会清空全部（created_at 均早于未来时间）。
        storage.CleanRecordsBefore(DateTime.UtcNow.AddDays(1));
        Assert.Empty(storage.GetRecords());
    }
}
