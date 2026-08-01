using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.Cli;

public static class Commands
{
    private static void Info(string message) => Console.WriteLine(message);
    private static void Warn(string message) => Console.Error.WriteLine($"[WARN] {message}");
    private static void Error(string message) => Console.Error.WriteLine($"[ERROR] {message}");

    private static CliSession NewSession(Args a, bool quiet = false, bool jsonEvents = false)
        => new(a.Option("db")) { Quiet = quiet, JsonEvents = jsonEvents };

    private static int ParseInt(string? text, int fallback)
        => int.TryParse(text, out var value) ? value : fallback;

    // ------------------------------------------------------------------ 扫描

    public static async Task<int> ScanAsync(Args a)
    {
        using var session = NewSession(a, quiet: true);
        var seconds = Math.Max(1, ParseInt(a.Option("seconds"), 8));
        var found = new Dictionary<string, (string Name, int Rssi, string LastSeen)>();
        session.Events.Subscribe<DeviceDiscoveredEvent>(e =>
            found[e.Addr] = (e.Name, e.Rssi, DateTime.Now.ToString("o")));

        session.Ble.StartScan();
        if (!a.Has("json"))
            Info($"正在扫描 BLE 设备（{seconds} 秒）...");
        await Task.Delay(TimeSpan.FromSeconds(seconds));
        session.Ble.StopScan();

        if (a.Has("json"))
        {
            Console.WriteLine(JsonSerializer.Serialize(found
                .Select(kv => new { addr = kv.Key, name = kv.Value.Name, rssi = kv.Value.Rssi, lastSeen = kv.Value.LastSeen })
                .OrderByDescending(x => x.rssi),
                new JsonSerializerOptions { WriteIndented = true }));
            return 0;
        }

        Info($"发现 {found.Count} 个设备：");
        foreach (var kv in found.OrderByDescending(x => x.Value.Rssi))
            Info($"  {Pad(kv.Value.Name, 24)} {kv.Key}  RSSI={kv.Value.Rssi} dBm");
        return 0;
    }

    // ------------------------------------------------------------------ 服务端

    public static async Task<int> ServeAsync(Args a)
    {
        using var session = NewSession(a, jsonEvents: a.Has("json"));
        var name = a.Option("name") ?? Environment.MachineName;
        var bleOnly = a.Has("ble-only");

        var gattOk = await session.GattServer.StartAsync(name);
        if (!gattOk)
        {
            Error("GATT 服务端启动失败（BLE 不可用或缺少蓝牙适配器）");
            return 1;
        }

        var rfcommOk = bleOnly;
        if (!bleOnly)
        {
            rfcommOk = await session.Rfcomm.StartServerAsync();
            if (!rfcommOk)
            {
                Error("RFCOMM 服务端启动失败（经典蓝牙不可用）");
                return 1;
            }
        }

        if (!a.Has("json"))
        {
            var nameNote = session.GattServer.IsNameAdvertised ? "" : "，本机名称广播不可用，可按地址连接";
            Info($"正在广播 \"{name}\"（BLE{(rfcommOk ? " + RFCOMM" : "")}{nameNote}），等待对端连接。Ctrl+C 退出。");
        }

        await WaitUntilCancelledOrTimeoutAsync(a.Option("timeout"));
        Info("已停止广播");
        return 0;
    }

    // ------------------------------------------------------------------ 连接

    public static async Task<int> ConnectAsync(Args a)
    {
        var addr = a.Get(1);
        if (string.IsNullOrWhiteSpace(addr))
        {
            Error("用法：btcli connect <设备地址> [--peer 名称] [--no-rfcomm] [--timeout 秒]");
            return 2;
        }

        using var session = NewSession(a, jsonEvents: a.Has("json"));
        if (!await EnsureConnectedAsync(session, addr, a.Option("peer"), !a.Has("no-rfcomm")))
            return 1;

        if (!a.Has("json"))
            Info("会话保持中（Ctrl+C 退出），对端发来的文本/文件会实时显示。");
        await WaitUntilCancelledOrTimeoutAsync(a.Option("timeout"));
        Info("已断开");
        return 0;
    }

    /// <summary>一次性命令共用：未连接则先建立 BLE（+可选 RFCOMM）连接。</summary>
    private static async Task<bool> EnsureConnectedAsync(CliSession session, string addr, string? peerName, bool rfcomm)
    {
        if (session.Ble.IsConnected)
        {
            if (!string.Equals(session.Ble.ConnectedAddr, addr, StringComparison.OrdinalIgnoreCase))
            {
                session.Ble.Disconnect();
                session.Rfcomm.Close();
            }
            else
            {
                return true;
            }
        }

        var ok = await session.Ble.ConnectAsync(addr);
        if (!ok)
        {
            Error($"BLE 连接失败：{addr}");
            return false;
        }

        if (rfcomm)
        {
            var rfcommOk = await session.Rfcomm.ConnectToServerAsync(peerName ?? "");
            if (!rfcommOk)
                Warn("RFCOMM 通道连接失败（大文件传输不可用）");
        }
        return true;
    }

    private static async Task WaitUntilCancelledOrTimeoutAsync(string? timeoutSeconds)
    {
        var cts = new CancellationTokenSource();
        Console.CancelKeyPress += (_, e) =>
        {
            e.Cancel = true;
            cts.Cancel();
        };
        try
        {
            var seconds = ParseInt(timeoutSeconds, 0);
            if (seconds > 0)
                await Task.Delay(TimeSpan.FromSeconds(seconds), cts.Token);
            else
                await Task.Delay(Timeout.InfiniteTimeSpan, cts.Token);
        }
        catch (TaskCanceledException)
        {
            // 用户按 Ctrl+C，正常退出。
        }
    }

    // ------------------------------------------------------------------ 发送

    public static async Task<int> SendTextAsync(Args a)
    {
        var addr = a.Get(1);
        var text = a.RemainingFrom(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(text))
        {
            Error("用法：btcli send-text <设备地址> <文本> [--peer 名称]");
            return 2;
        }

        using var session = NewSession(a, quiet: true);
        if (!await EnsureConnectedAsync(session, addr, a.Option("peer"), rfcomm: false))
            return 1;

        var ok = await session.Ble.SendTextAsync(text);
        await Task.Delay(300);
        if (ok)
            Info($"文本已发送（{Encoding.UTF8.GetByteCount(text)} 字节）");
        else
            Error("文本发送失败");
        return ok ? 0 : 1;
    }

    public static async Task<int> SendFileAsync(Args a)
    {
        var addr = a.Get(1);
        var path = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(path))
        {
            Error("用法：btcli send-file <设备地址> <文件路径> [--peer 名称] [--compress|--no-compress] [--encrypt|--no-encrypt] [--chunk 字节]");
            return 2;
        }
        if (!File.Exists(path))
        {
            Error($"文件不存在：{path}");
            return 2;
        }

        using var session = NewSession(a, quiet: true);
        if (!await EnsureConnectedAsync(session, addr, a.Option("peer"), rfcomm: true))
            return 1;

        var previous = ApplyTransferOverrides(session.Config, a);
        try
        {
            var ok = await session.FileTransfer.SendFileAsync(path);
            await Task.Delay(500);
            if (ok)
                Info($"文件已发送：{Path.GetFileName(path)}");
            else
                Error($"文件发送失败：{Path.GetFileName(path)}");
            return ok ? 0 : 1;
        }
        finally
        {
            RestoreTransferOverrides(session.Config, previous);
        }
    }

    public static async Task<int> SendFolderAsync(Args a)
    {
        var addr = a.Get(1);
        var folder = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(folder))
        {
            Error("用法：btcli send-folder <设备地址> <文件夹路径> [--peer 名称] [--compress|--no-compress] [--encrypt|--no-encrypt]");
            return 2;
        }
        if (!Directory.Exists(folder))
        {
            Error($"文件夹不存在：{folder}");
            return 2;
        }

        using var session = NewSession(a, quiet: true);
        if (!await EnsureConnectedAsync(session, addr, a.Option("peer"), rfcomm: true))
            return 1;

        var previous = ApplyTransferOverrides(session.Config, a);
        try
        {
            var ok = await session.FileTransfer.SendFolderAsync(folder);
            await Task.Delay(500);
            if (ok)
                Info($"文件夹已发送：{folder}");
            else
                Error($"文件夹发送失败：{folder}");
            return ok ? 0 : 1;
        }
        finally
        {
            RestoreTransferOverrides(session.Config, previous);
        }
    }

    private static (bool Compress, bool Encrypt, int Chunk) ApplyTransferOverrides(AppConfig config, Args a)
    {
        var previous = (config.CompressionEnabled, config.EncryptionEnabled, config.RfcommChunkSize);
        if (a.Has("compress")) config.CompressionEnabled = true;
        if (a.Has("no-compress")) config.CompressionEnabled = false;
        if (a.Has("encrypt")) config.EncryptionEnabled = true;
        if (a.Has("no-encrypt")) config.EncryptionEnabled = false;
        if (a.Option("chunk") is { } chunkText && int.TryParse(chunkText, out var chunk) && chunk > 0)
            config.RfcommChunkSize = chunk;
        return previous;
    }

    private static void RestoreTransferOverrides(AppConfig config, (bool Compress, bool Encrypt, int Chunk) previous)
    {
        config.CompressionEnabled = previous.Compress;
        config.EncryptionEnabled = previous.Encrypt;
        config.RfcommChunkSize = previous.Chunk;
    }

    // ------------------------------------------------------------------ OPP 通用推送（1.1）

    public static async Task<int> OppScanAsync(Args a)
    {
        using var session = NewSession(a, quiet: true);
        var seconds = Math.Max(0, ParseInt(a.Option("seconds"), 5));
        var pairedOnly = a.Has("paired-only");
        if (!a.Has("json"))
            Info($"正在扫描支持 OPP（蓝牙文件接收）的设备（{(pairedOnly ? "仅已配对" : "含可发现")}，{seconds} 秒）...");

        var devices = await session.OppDiscovery.DiscoverAsync(pairedOnly, seconds);

        if (a.Has("json"))
        {
            Console.WriteLine(JsonSerializer.Serialize(devices
                .Select(d => new { addr = d.Addr, name = d.Name, paired = d.IsPaired })
                .OrderByDescending(x => x.paired).ThenBy(x => x.name),
                new JsonSerializerOptions { WriteIndented = true }));
            return 0;
        }

        Info($"发现 {devices.Count} 个 OPP 设备：");
        if (devices.Count == 0)
            Info("  （无结果：请确认对端已开启蓝牙且处于可发现状态，或先在 Windows 设置中完成配对）");
        foreach (var d in devices.OrderByDescending(x => x.IsPaired).ThenBy(x => x.Name))
            Info($"  {Pad(d.Name, 28)} {d.AddrDisplay}   {(d.IsPaired ? "已配对" : "未配对")}");
        return 0;
    }

    public static async Task<int> OppPairAsync(Args a)
    {
        var addr = a.Get(1);
        if (string.IsNullOrWhiteSpace(addr))
        {
            Error("用法：btcli opp-pair <设备地址> [--pin 1234] [--db 路径]");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var ok = await session.OppDiscovery.PairAsync(addr, a.Option("pin"));
        Info(ok ? $"配对成功：{addr}" : $"配对失败：{addr}（可尝试在系统设置中手动配对）");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendFileAsync(Args a)
    {
        var addr = a.Get(1);
        var path = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(path))
        {
            Error("用法：btcli opp-send-file <设备地址> <文件> [--db 路径]");
            return 2;
        }
        if (!File.Exists(path))
        {
            Error($"文件不存在：{path}");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var ok = await session.OppPush.SendFileAsync(addr, path);
        Info(ok ? $"文件已通过 OPP 推送：{Path.GetFileName(path)}" : $"OPP 推送失败：{Path.GetFileName(path)}");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendTextAsync(Args a)
    {
        var addr = a.Get(1);
        var text = a.RemainingFrom(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(text))
        {
            Error("用法：btcli opp-send-text <设备地址> <文本> [--name 文件名] [--db 路径]");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var ok = await session.OppPush.SendTextAsync(addr, text, a.Option("name"));
        Info(ok
            ? $"文本已通过 OPP 推送（{Encoding.UTF8.GetByteCount(text)} 字节）"
            : "OPP 文本推送失败");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendFolderAsync(Args a)
    {
        var addr = a.Get(1);
        var folder = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(folder))
        {
            Error("用法：btcli opp-send-folder <设备地址> <文件夹> [--db 路径]");
            return 2;
        }
        if (!Directory.Exists(folder))
        {
            Error($"文件夹不存在：{folder}");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var ok = await session.OppPush.SendFolderAsync(addr, folder);
        Info(ok ? $"文件夹已压缩并通过 OPP 推送：{folder}" : "OPP 文件夹推送失败");
        return ok ? 0 : 1;
    }

    // ------------------------------------------------------------------ 记录 / 配置

    public static int Records(Args a)
    {
        using var session = NewSession(a, quiet: true);
        var sub = (a.Get(1) ?? "").ToLowerInvariant();
        if (sub == "clear")
        {
            if (!a.Has("yes"))
            {
                var count = session.Storage.GetRecords(limit: 100000).Count;
                Console.Error.Write($"确定要清空全部 {count} 条传输记录吗？该操作不可恢复。输入 yes 确认：");
                var answer = Console.ReadLine()?.Trim();
                if (!string.Equals(answer, "yes", StringComparison.OrdinalIgnoreCase))
                {
                    Info("已取消");
                    return 1;
                }
            }
            var deleted = session.Storage.ClearRecords();
            Info($"已清空 {deleted} 条传输记录");
            return 0;
        }

        var records = session.Storage.GetRecords(
            direction: a.Option("direction"),
            type: a.Option("type"),
            peerAddr: a.Option("peer"),
            status: a.Option("status"),
            from: null,
            to: null,
            search: a.Option("search"),
            orderBy: "created_at DESC",
            limit: Math.Clamp(ParseInt(a.Option("limit"), 100), 1, 100000));

        if (a.Has("json"))
        {
            Console.WriteLine(JsonSerializer.Serialize(records.Select(r => new
            {
                id = r.Id,
                createdAt = r.CreatedAt,
                direction = r.Direction,
                type = r.Type,
                peerName = r.PeerName,
                peerAddr = r.PeerAddr,
                name = r.Name,
                size = r.Size,
                status = r.Status,
                checksum = r.Checksum,
                channel = r.Channel,
                localPath = r.LocalPath,
                note = r.Note
            }), new JsonSerializerOptions { WriteIndented = true }));
            return 0;
        }

        if (records.Count == 0)
        {
            Info("无记录");
            return 0;
        }

        var header = new[] { "ID", "时间", "方向", "类型", "对端", "名称", "大小", "状态", "通道", "本地路径" };
        var rows = records.Select(r => new[]
        {
            r.Id.ToString(),
            r.CreatedAtDisplay,
            r.DirectionDisplay,
            r.TypeDisplay,
            r.PeerName,
            Truncate(r.Name, 32),
            r.Size.ToString("N0"),
            r.StatusDisplay,
            r.ChannelDisplay,
            Truncate(r.LocalPath, 60)
        }).ToList();
        PrintTable(header, rows);
        return 0;
    }

    public static int Export(Args a)
    {
        var format = (a.Get(1) ?? "").ToLowerInvariant();
        var path = a.Get(2);
        if (format is not ("csv" or "json") || string.IsNullOrEmpty(path))
        {
            Error("用法：btcli export <csv|json> <输出文件>");
            return 2;
        }

        using var session = NewSession(a, quiet: true);
        var records = session.Storage.GetRecords(limit: 100000);
        var file = format == "csv"
            ? ExportService.ExportCsv(records, path)
            : ExportService.ExportJson(records, path);
        Info($"已导出 {records.Count} 条记录到 {file}");
        return 0;
    }

    public static int Stats(Args a)
    {
        using var session = NewSession(a, quiet: true);
        var (totalCount, totalBytes, sendCount, recvCount) = session.Storage.GetStats();
        if (a.Has("json"))
        {
            Console.WriteLine(JsonSerializer.Serialize(new { totalCount, totalBytes, sendCount, recvCount }));
            return 0;
        }
        Info($"总记录：{totalCount}    总字节：{totalBytes:N0}    发送：{sendCount}    接收：{recvCount}");
        return 0;
    }

    public static int Devices(Args a)
    {
        using var session = NewSession(a, quiet: true);
        var devices = session.Storage.GetDevices();
        if (a.Has("json"))
        {
            Console.WriteLine(JsonSerializer.Serialize(devices.Select(d => new
            {
                addr = d.Addr,
                name = d.Name,
                alias = d.Alias,
                favorite = d.Favorite,
                lastSeen = d.LastSeen,
                lastConnected = d.LastConnected
            }), new JsonSerializerOptions { WriteIndented = true }));
            return 0;
        }
        if (devices.Count == 0)
        {
            Info("无已记录设备");
            return 0;
        }
        var header = new[] { "地址", "名称", "别名", "收藏", "最近连接" };
        var rows = devices.Select(d => new[]
        {
            d.Addr,
            d.Name,
            d.Alias,
            d.Favorite ? "是" : "否",
            d.LastConnected
        }).ToList();
        PrintTable(header, rows);
        return 0;
    }

    public static int Config(Args a)
    {
        using var session = NewSession(a, quiet: true);
        var sub = (a.Get(1) ?? "").ToLowerInvariant();
        if (sub == "set")
        {
            var key = a.Get(2);
            var value = a.Get(3);
            if (string.IsNullOrEmpty(key) || value == null)
            {
                Error("用法：btcli config set <key> <value>");
                return 2;
            }
            if (!SetConfigValue(session.Config, key, value))
            {
                Error($"未知配置项：{key}（可选 RecvDirectory/AutoCopyClipboard/CompressionEnabled/EncryptionEnabled/RfcommChunkSize/OppChunkSize/OppConnectTimeout/OppSendTimeout/PushTextFileName/OppAuthPassword/OppNameUseBom/OppProtectionLevel）");
                return 2;
            }
            session.Config.Save();
            Info($"已保存：{key} = {value}");
            return 0;
        }

        Info($"接收目录       RecvDirectory        = {session.Config.RecvDirectory}");
        Info($"自动复制剪贴板 AutoCopyClipboard    = {session.Config.AutoCopyClipboard}");
        Info($"启用压缩       CompressionEnabled   = {session.Config.CompressionEnabled}");
        Info($"启用加密       EncryptionEnabled    = {session.Config.EncryptionEnabled}");
        Info($"RFCOMM 分块    RfcommChunkSize      = {session.Config.RfcommChunkSize}");
        Info($"OPP 分块       OppChunkSize          = {session.Config.OppChunkSize}");
        Info($"OPP 连接超时    OppConnectTimeout     = {session.Config.OppConnectTimeoutSeconds} 秒");
        Info($"OPP 发送超时    OppSendTimeout        = {session.Config.OppSendTimeoutSeconds} 秒");
        Info($"OPP 文本文件名  PushTextFileName       = {session.Config.PushTextFileName}");
        Info($"OPP 认证密码    OppAuthPassword       = {(string.IsNullOrEmpty(session.Config.OppAuthPassword) ? "（未设置）" : "***")}");
        Info($"OPP Name BOM    OppNameUseBom         = {session.Config.OppNameUseBom}");
        Info($"OPP 保护级别    OppProtectionLevel    = {session.Config.OppProtectionLevel}（auto/plain/encrypt）");
        return 0;
    }

    private static bool SetConfigValue(AppConfig config, string key, string value)
    {
        switch (key.Trim().ToLowerInvariant())
        {
            case "recvdir" or "recvdirectory" or "接收目录":
                config.RecvDirectory = value;
                return true;
            case "autocopy" or "autocopyclipboard" or "自动复制":
                if (!bool.TryParse(value, out var autoCopy)) return false;
                config.AutoCopyClipboard = autoCopy;
                return true;
            case "compression" or "compressionenabled" or "压缩":
                if (!bool.TryParse(value, out var compression)) return false;
                config.CompressionEnabled = compression;
                return true;
            case "encryption" or "encryptionenabled" or "加密":
                if (!bool.TryParse(value, out var encryption)) return false;
                config.EncryptionEnabled = encryption;
                return true;
            case "rfcommchunk" or "rfcommchunksize" or "分块":
                if (!int.TryParse(value, out var chunk) || chunk <= 0) return false;
                config.RfcommChunkSize = chunk;
                return true;
            case "oppchunk" or "oppchunksize":
                if (!int.TryParse(value, out var oppChunk) || oppChunk <= 0) return false;
                config.OppChunkSize = oppChunk;
                return true;
            case "oppconnecttimeout":
                if (!int.TryParse(value, out var connectTimeout) || connectTimeout <= 0) return false;
                config.OppConnectTimeoutSeconds = connectTimeout;
                return true;
            case "oppsendtimeout":
                if (!int.TryParse(value, out var sendTimeout) || sendTimeout <= 0) return false;
                config.OppSendTimeoutSeconds = sendTimeout;
                return true;
            case "pushtextfilename":
                if (string.IsNullOrWhiteSpace(value)) return false;
                config.PushTextFileName = value;
                return true;
            case "oppauthpassword":
                config.OppAuthPassword = value;
                return true;
            case "oppnameusebom" or "oppbom":
                if (!bool.TryParse(value, out var useBom)) return false;
                config.OppNameUseBom = useBom;
                return true;
            case "oppprotectionlevel" or "oppprotection":
                if (value is not ("auto" or "plain" or "encrypt")) return false;
                config.OppProtectionLevel = value;
                return true;
            default:
                return false;
        }
    }

    // ------------------------------------------------------------------ 自检

    public static async Task<int> SelftestAsync(Args _)
    {
        var results = new List<(string Name, bool Ok, string Detail)>();

        results.Add(TestFraming());
        results.Add(TestCrypto());
        results.Add(TestCompression());
        results.Add(TestStorageAndExport());
        results.Add(await TestObexAsync());

        var failed = results.Count(r => !r.Ok);
        Console.WriteLine();
        Console.WriteLine("=== 自检结果 ===");
        foreach (var (name, ok, detail) in results)
            Console.WriteLine($"  [{(ok ? "PASS" : "FAIL")}] {name}{(detail.Length > 0 ? "：" + detail : "")}");
        Console.WriteLine(failed == 0 ? "全部通过 ✔" : $"{failed} 项失败 ✘");
        return failed == 0 ? 0 : 1;
    }

    private static async Task<(string Name, bool Ok, string Detail)> TestObexAsync()
    {
        try
        {
            // 1) Name 头编码往返（中文/emoji）
            var nameHeader = ObexHeader.Name("中文文件 测试 😀.txt");
            if (nameHeader.AsName() != "中文文件 测试 😀.txt")
                return ("OBEX 协议（编解码/流程）", false, "Name 头往返不一致");

            // 2) 超长名称截断（UTF-16 字节数 > 255）
            var longHeader = ObexHeader.Name(new string('汉', 200) + ".txt");
            var decoded = longHeader.AsName();
            if (decoded == null || decoded.Length == 0 || longHeader.Value.Length > 259)
                return ("OBEX 协议（编解码/流程）", false, "长名称截断失败");

            // 3) PUT 包往返
            var pkt = new ObexPacket { Opcode = (byte)ObexRequestOpcode.Put };
            pkt.Headers.Add(ObexHeader.Name("a.txt"));
            pkt.Headers.Add(ObexHeader.Type("application/octet-stream"));
            pkt.Headers.Add(ObexHeader.LengthHeader(5));
            pkt.Headers.Add(ObexHeader.Body(new byte[] { 1, 2, 3, 4, 5 }, true));
            var parsed = ObexPacket.Deserialize(pkt.ToBytes(), out var error);
            if (parsed == null || error != null || parsed.Headers.Count != 4)
                return ("OBEX 协议（编解码/流程）", false, "PUT 包往返失败");

            // 4) 篡改长度字段必须被拒绝
            var raw = pkt.ToBytes();
            raw[1] = 0xFF;
            raw[2] = 0xFE;
            if (ObexPacket.Deserialize(raw, out _) != null)
                return ("OBEX 协议（编解码/流程）", false, "非法长度字段未被拒绝");

            // 5) 假传输完整 PUT 流程（CONNECT -> 分片 PUT -> DISCONNECT）
            var data = new byte[70000];
            for (var i = 0; i < data.Length; i++) data[i] = (byte)(i % 251);
            var transport = new MemoryObexTransport { AutoReply = true };
            await using var client = new ObexClient(transport);
            await client.ConnectAsync();
            using var ms = new MemoryStream(data);
            var result = await client.PushAsync("big.bin", "application/octet-stream", data.Length,
                (offset, buffer, count, token) =>
                {
                    ms.Seek(offset, SeekOrigin.Begin);
                    var n = ms.Read(buffer, 0, count);
                    return Task.FromResult(n);
                }, 32768);
            await client.DisconnectAsync();

            if (result.BytesSent != data.Length)
                return ("OBEX 协议（编解码/流程）", false, $"分片字节数不符：{result.BytesSent} != {data.Length}");
            var packets = transport.ParseWrittenPackets();
            if (packets == null || packets.Count < 2 ||
                !packets.Any(p => p.Headers.Any(h => h.Id == ObexHeaderId.EndOfBody)))
                return ("OBEX 协议（编解码/流程）", false, "缺少 EndOfBody 头或包序列不完整");

            return ("OBEX 协议（编解码/流程）", true,
                $"编解码/截断/校验/假传输流程（{data.Length} 字节，{packets.Count} 包）全部通过");
        }
        catch (Exception ex)
        {
            return ("OBEX 协议（编解码/流程）", false, ex.Message);
        }
    }

    private static (string Name, bool Ok, string Detail) TestFraming()
    {
        var ok = true;
        var detail = "";
        foreach (var size in new[] { 0, 1, 20, 4096, 60000 })
        {
            var payload = size == 0
                ? Array.Empty<byte>()
                : Enumerable.Range(0, size).Select(i => (byte)(i % 251)).ToArray();
            var frame = new Frame
            {
                MsgType = MsgType.DATA,
                TaskId = 0xDEADBEEF,
                SeqNo = 12345,
                TotalLen = (uint)size,
                Offset = 100,
                Flags = FrameFlags.FinalChunk | FrameFlags.Compressed,
                Payload = payload
            };
            var parsed = Frame.Deserialize(frame.Serialize());
            if (parsed == null || parsed.MsgType != frame.MsgType || parsed.TaskId != frame.TaskId ||
                parsed.SeqNo != frame.SeqNo || parsed.TotalLen != frame.TotalLen || parsed.Offset != frame.Offset ||
                parsed.Flags != frame.Flags || !parsed.Payload.SequenceEqual(payload))
            {
                ok = false;
                detail = $"size={size} 往返不一致";
                break;
            }
        }

        if (ok)
        {
            var frame = new Frame { MsgType = MsgType.META, TaskId = 7, TotalLen = 3, Payload = new byte[] { 1, 2, 3 } };
            var raw = frame.Serialize();
            raw[^1] ^= 0xFF;
            if (Frame.Deserialize(raw) != null)
            {
                ok = false;
                detail = "CRC 未拦截篡改帧";
            }
        }

        return ("协议分帧 / CRC16", ok, ok ? "多尺寸往返一致，篡改帧被拒绝" : detail);
    }

    private static (string Name, bool Ok, string Detail) TestCrypto()
    {
        try
        {
            var alice = new CryptoService();
            var bob = new CryptoService();
            var alicePub = alice.GetPublicKey();
            var bobPub = bob.GetPublicKey();
            alice.DeriveSessionKey(bobPub);
            bob.DeriveSessionKey(alicePub);

            var plain = Encoding.UTF8.GetBytes("蓝牙传输 CLI 自检 AES-GCM 0123456789");
            var cipher = alice.Encrypt(plain);
            var roundTrip = bob.Decrypt(cipher).SequenceEqual(plain);

            var tampered = (byte[])cipher.Clone();
            tampered[^1] ^= 0x01;
            var tamperRejected = false;
            try
            {
                bob.Decrypt(tampered);
            }
            catch (CryptographicException)
            {
                tamperRejected = true;
            }

            return ("加密（ECDH + AES-GCM）", roundTrip && tamperRejected,
                roundTrip && tamperRejected ? "密钥协商成功，加解密一致，篡改被拒绝" : "往返或篡改检测失败");
        }
        catch (Exception ex)
        {
            return ("加密（ECDH + AES-GCM）", false, ex.Message);
        }
    }

    private static (string Name, bool Ok, string Detail) TestCompression()
    {
        try
        {
            var data = new byte[200_000];
            for (var i = 0; i < data.Length; i++)
                data[i] = (byte)(i % 97);
            var compressed = RfcommChannel.CompressData(data);
            var restored = RfcommChannel.DecompressData(compressed);
            var ok = restored.SequenceEqual(data) && compressed.Length < data.Length;
            return ("Deflate 压缩", ok,
                ok ? $"200KB -> {compressed.Length} 字节，往返一致" : "往返不一致或未压缩");
        }
        catch (Exception ex)
        {
            return ("Deflate 压缩", false, ex.Message);
        }
    }

    private static (string Name, bool Ok, string Detail) TestStorageAndExport()
    {
        var dir = Path.Combine(Path.GetTempPath(), "btcli_selftest_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        try
        {
            var dbPath = Path.Combine(dir, "selftest.db");
            var storage = new StorageService(dbPath);
            storage.AddRecord(new TransferRecord
            {
                Direction = TransferConst.DirSend, Type = TransferConst.TypeText,
                PeerName = "自检", Name = "你好", Size = 6, Status = TransferConst.StatusOk,
                Channel = TransferConst.ChannelBle
            });
            storage.AddRecord(new TransferRecord
            {
                Direction = TransferConst.DirRecv, Type = TransferConst.TypeFile,
                PeerName = "自检", Name = "a.bin", Size = 1024, Status = TransferConst.StatusOk,
                Channel = TransferConst.ChannelRfcomm, LocalPath = Path.Combine(dir, "a.bin")
            });

            var records = storage.GetRecords();
            var stats = storage.GetStats();
            var csv = ExportService.ExportCsv(records, Path.Combine(dir, "out.csv"));
            var json = ExportService.ExportJson(records, Path.Combine(dir, "out.json"));
            var csvOk = File.ReadAllText(csv).Contains("Id,CreatedAt");
            var jsonOk = File.ReadAllText(json).Contains("\"Direction\"");
            var crudOk = records.Count == 2 && stats.totalCount == 2 && stats.sendCount == 1 &&
                         stats.recvCount == 1 && csvOk && jsonOk;
            storage.DeleteRecord(records[0].Id);
            crudOk &= storage.GetRecords().Count == 1;
            var cleared = storage.ClearRecords();
            crudOk &= cleared == 1 && storage.GetRecords().Count == 0 && storage.GetStats().totalCount == 0;

            return ("存储 / 导出", crudOk,
                crudOk ? "写入/查询/统计/CSV/JSON/删除/清空全部通过" : "存储链路校验失败");
        }
        catch (Exception ex)
        {
            return ("存储 / 导出", false, ex.Message);
        }
        finally
        {
            try
            {
                foreach (var file in Directory.GetFiles(dir))
                    File.Delete(file);
                Directory.Delete(dir);
            }
            catch
            {
                // 清理失败不影响自检结论。
            }
        }
    }

    // ------------------------------------------------------------------ REPL

    public static async Task<int> ReplAsync(Args a)
    {
        using var session = NewSession(a);
        var replDb = a.Option("db");
        Info("btcli 交互模式 —— 输入 help 查看命令，quit 退出。");
        while (true)
        {
            Console.Write("btcli> ");
            var line = Console.ReadLine();
            if (line == null) break;
            line = line.Trim();
            if (line.Length == 0) continue;

            var tokens = line.Split(' ', StringSplitOptions.RemoveEmptyEntries);
            // 会话指定了独立数据库时，让 records/stats/export 等子命令沿用同一数据库。
            if (replDb != null && !tokens.Any(t => t.StartsWith("--db", StringComparison.OrdinalIgnoreCase)))
                tokens = tokens.Concat(new[] { "--db", replDb }).ToArray();
            var parts = Args.Parse(tokens);
            var command = (parts.Get(0) ?? "").ToLowerInvariant();
            try
            {
                switch (command)
                {
                    case "help" or "?":
                        ReplHelp();
                        break;
                    case "quit" or "exit":
                        // Dispose 会统一清理连接，避免重复关闭打印两次日志。
                        return 0;
                    case "clear":
                        Console.Clear();
                        break;
                    case "scan":
                        await ReplScanAsync(session, parts);
                        break;
                    case "serve":
                        await ReplServeAsync(session, parts);
                        break;
                    case "stop":
                        session.GattServer.Stop();
                        session.Rfcomm.Close();
                        Info("已停止广播/服务");
                        break;
                    case "connect":
                        await ReplConnectAsync(session, parts);
                        break;
                    case "disconnect":
                        session.Ble.Disconnect();
                        session.Rfcomm.Close();
                        Info("已断开");
                        break;
                    case "send-text":
                        await ReplSendTextAsync(session, parts);
                        break;
                    case "send-file":
                        await ReplSendFileAsync(session, parts);
                        break;
                    case "send-folder":
                        await ReplSendFolderAsync(session, parts);
                        break;
                    case "records":
                        Records(parts);
                        break;
                    case "stats":
                        Stats(parts);
                        break;
                    case "devices":
                        Devices(parts);
                        break;
                    case "config":
                        Config(parts);
                        break;
                    case "export":
                        Export(parts);
                        break;
                    default:
                        Error($"未知命令：{command}（输入 help 查看帮助）");
                        break;
                }
            }
            catch (Exception ex)
            {
                Error(ex.Message);
            }
        }
        return 0;
    }

    private static async Task ReplScanAsync(CliSession session, Args parts)
    {
        var seconds = Math.Max(1, ParseInt(parts.Option("seconds"), 8));
        var found = new List<(string Addr, string Name, int Rssi)>();
        session.Events.Subscribe<DeviceDiscoveredEvent>(e =>
        {
            var index = found.FindIndex(x => x.Addr == e.Addr);
            if (index >= 0)
                found[index] = (e.Addr, e.Name, e.Rssi);
            else
                found.Add((e.Addr, e.Name, e.Rssi));
        });

        session.Ble.StartScan();
        Info($"正在扫描 BLE 设备（{seconds} 秒）...");
        await Task.Delay(TimeSpan.FromSeconds(seconds));
        session.Ble.StopScan();

        foreach (var (addr, name, rssi) in found.OrderByDescending(x => x.Rssi))
            Info($"  {Pad(name, 24)} {addr}  RSSI={rssi} dBm");
    }

    private static async Task ReplServeAsync(CliSession session, Args parts)
    {
        var name = parts.Option("name") ?? Environment.MachineName;
        var bleOnly = parts.Has("ble-only");
        var gattOk = await session.GattServer.StartAsync(name);
        if (!gattOk)
        {
            Error("GATT 服务端启动失败（BLE 不可用或缺少蓝牙适配器）");
            return;
        }

        var rfcommOk = bleOnly;
        if (!bleOnly)
        {
            rfcommOk = await session.Rfcomm.StartServerAsync();
            if (!rfcommOk)
            {
                session.GattServer.Stop();
                Error("RFCOMM 服务端启动失败");
                return;
            }
        }
        Info($"正在广播 \"{name}\"（BLE{(rfcommOk ? " + RFCOMM" : "")}），输入 stop 停止。");
    }

    private static async Task ReplConnectAsync(CliSession session, Args parts)
    {
        var addr = parts.Get(1);
        if (string.IsNullOrWhiteSpace(addr))
        {
            Error("用法：connect <设备地址> [--peer 名称] [--no-rfcomm]");
            return;
        }

        if (session.Ble.IsConnected &&
            !string.Equals(session.Ble.ConnectedAddr, addr, StringComparison.OrdinalIgnoreCase))
        {
            session.Ble.Disconnect();
            session.Rfcomm.Close();
        }

        if (!await session.Ble.ConnectAsync(addr))
        {
            Error($"BLE 连接失败：{addr}");
            return;
        }

        if (!parts.Has("no-rfcomm"))
        {
            var rfcommOk = await session.Rfcomm.ConnectToServerAsync(parts.Option("peer") ?? "");
            if (!rfcommOk)
                Warn("RFCOMM 通道连接失败（大文件传输不可用）");
        }
        Info("已连接");
    }

    private static async Task ReplSendTextAsync(CliSession session, Args parts)
    {
        if (!session.Ble.IsConnected)
        {
            Error("尚未连接，请先 connect <地址>");
            return;
        }
        var text = parts.RemainingFrom(1);
        if (string.IsNullOrWhiteSpace(text))
        {
            Error("用法：send-text <文本>");
            return;
        }
        var ok = await session.Ble.SendTextAsync(text);
        Info(ok ? $"文本已发送（{Encoding.UTF8.GetByteCount(text)} 字节）" : "发送失败");
    }

    private static async Task ReplSendFileAsync(CliSession session, Args parts)
    {
        if (!session.Ble.IsConnected)
        {
            Error("尚未连接，请先 connect <地址>");
            return;
        }
        var path = parts.Get(1);
        if (string.IsNullOrEmpty(path) || !File.Exists(path))
        {
            Error("用法：send-file <文件路径>");
            return;
        }
        var previous = ApplyTransferOverrides(session.Config, parts);
        try
        {
            var ok = await session.FileTransfer.SendFileAsync(path);
            Info(ok ? $"文件已发送：{Path.GetFileName(path)}" : $"文件发送失败：{Path.GetFileName(path)}");
        }
        finally
        {
            RestoreTransferOverrides(session.Config, previous);
        }
    }

    private static async Task ReplSendFolderAsync(CliSession session, Args parts)
    {
        if (!session.Ble.IsConnected)
        {
            Error("尚未连接，请先 connect <地址>");
            return;
        }
        var folder = parts.Get(1);
        if (string.IsNullOrEmpty(folder) || !Directory.Exists(folder))
        {
            Error("用法：send-folder <文件夹路径>");
            return;
        }
        var previous = ApplyTransferOverrides(session.Config, parts);
        try
        {
            var ok = await session.FileTransfer.SendFolderAsync(folder);
            Info(ok ? $"文件夹已发送：{folder}" : $"文件夹发送失败：{folder}");
        }
        finally
        {
            RestoreTransferOverrides(session.Config, previous);
        }
    }

    private static void ReplHelp()
    {
        Info("""
            help                    显示本帮助
            scan [seconds]          扫描 BLE 设备
            serve [--name 名称] [--ble-only]
                                  启动接收端（BLE + RFCOMM），stop 停止
            stop                    停止广播/服务
            connect <地址> [--peer 名称] [--no-rfcomm]
                                  连接对端
            disconnect              断开连接
            send-text <文本>        发送文本（需已连接）
            send-file <路径>        发送文件（需已连接，支持 --compress/--encrypt/--chunk）
            send-folder <路径>      发送文件夹
            records / stats / devices / config / export <csv|json> <路径>
                                  查询记录、统计、设备、配置与导出
            quit / exit             退出
            """);
    }

    // ------------------------------------------------------------------ 表格工具

    private static void PrintTable(string[] header, List<string[]> rows)
    {
        var columns = header.Length;
        var widths = new int[columns];
        for (var c = 0; c < columns; c++)
        {
            widths[c] = DisplayWidth(header[c]);
            foreach (var row in rows)
                widths[c] = Math.Max(widths[c], DisplayWidth(row[c]));
        }

        Console.WriteLine(string.Join("  ", header.Select((h, c) => Pad(h, widths[c]))));
        Console.WriteLine(string.Join("  ", widths.Select(w => new string('-', w))));
        foreach (var row in rows)
            Console.WriteLine(string.Join("  ", row.Select((cell, c) => Pad(cell, widths[c]))));
    }

    private static string Truncate(string value, int maxWidth)
    {
        if (DisplayWidth(value) <= maxWidth) return value;
        var result = new StringBuilder();
        var width = 0;
        foreach (var ch in value)
        {
            var w = ch > 127 ? 2 : 1;
            if (width + w > maxWidth - 1) break;
            result.Append(ch);
            width += w;
        }
        return result.ToString().TrimEnd() + "…";
    }

    private static int DisplayWidth(string value) => value.Sum(c => c > 127 ? 2 : 1);

    private static string Pad(string value, int width)
    {
        var padding = Math.Max(0, width - DisplayWidth(value));
        return value + new string(' ', padding);
    }
}
