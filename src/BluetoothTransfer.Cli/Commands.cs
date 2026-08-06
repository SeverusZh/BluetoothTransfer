using System.IO.Compression;
using System.Text;
using System.Text.Json;
using BluetoothTransfer.Core.Discovery;
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

    // ------------------------------------------------------------------ OPP 通用推送

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
            Error("用法：btcli opp-send-file <设备地址> <文件> [--zip] [--mode auto|assistant|opp] [--db 路径]");
            return 2;
        }
        if (!File.Exists(path))
        {
            Error($"文件不存在：{path}");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var mode = (a.Option("mode") ?? "opp").ToLowerInvariant();
        var ok = mode switch
        {
            "assistant" => await session.AssistantPush.SendFileAsync(addr, path, zip: a.Has("zip")),
            "auto" => await session.AssistantPush.SendFileAsync(addr, path, zip: a.Has("zip")),
            _ => await session.OppPush.SendFileAsync(addr, path, zip: a.Has("zip"))
        };
        Info(ok ? $"文件已推送（{(mode == "opp" ? "OPP" : "助手")}）：{Path.GetFileName(path)}" : $"推送失败：{Path.GetFileName(path)}");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendTextAsync(Args a)
    {
        var addr = a.Get(1);
        var text = a.RemainingFrom(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(text))
        {
            Error("用法：btcli opp-send-text <设备地址> <文本> [--name 文件名] [--mode auto|assistant|opp] [--db 路径]");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var mode = (a.Option("mode") ?? "opp").ToLowerInvariant();
        var ok = mode switch
        {
            "assistant" => await session.AssistantPush.SendTextAsync(addr, text, a.Option("name")),
            "auto" => await session.AssistantPush.SendTextAsync(addr, text, a.Option("name")),
            _ => await session.OppPush.SendTextAsync(addr, text, a.Option("name"))
        };
        Info(ok
            ? $"文本已推送（{(mode == "opp" ? "OPP" : "助手")}，{Encoding.UTF8.GetByteCount(text)} 字节）"
            : "文本推送失败");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendFolderAsync(Args a)
    {
        var addr = a.Get(1);
        var folder = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(folder))
        {
            Error("用法：btcli opp-send-folder <设备地址> <文件夹> [--mode auto|assistant|opp] [--db 路径]");
            return 2;
        }
        if (!Directory.Exists(folder))
        {
            Error($"文件夹不存在：{folder}");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var mode = (a.Option("mode") ?? "opp").ToLowerInvariant();
        var ok = mode switch
        {
            "assistant" => await session.AssistantPush.SendFolderAsync(addr, folder),
            "auto" => await session.AssistantPush.SendFolderAsync(addr, folder),
            _ => await session.OppPush.SendFolderAsync(addr, folder)
        };
        Info(ok ? $"文件夹已压缩并推送（{(mode == "opp" ? "OPP" : "助手")}）：{folder}" : "文件夹推送失败");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendFilesAsync(Args a)
    {
        var addr = a.Get(1);
        var paths = new List<string>();
        for (var i = 2; ; i++)
        {
            var p = a.Get(i);
            if (string.IsNullOrEmpty(p)) break;
            paths.Add(p);
        }
        if (string.IsNullOrWhiteSpace(addr) || paths.Count == 0)
        {
            Error("用法：btcli opp-send-files <设备地址> <文件1> [文件2 ...] [--zip] [--db 路径]");
            return 2;
        }
        var existing = paths.Where(File.Exists).ToList();
        if (existing.Count == 0)
        {
            Error("没有可发送的文件（路径不存在）");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var mode = (a.Option("mode") ?? "opp").ToLowerInvariant();
        var okCount = 0;
        var failed = new List<string>();
        foreach (var path in existing)
        {
            var ok = mode switch
            {
                "assistant" => await session.AssistantPush.SendFileAsync(addr, path, zip: a.Has("zip")),
                "auto" => await session.AssistantPush.SendFileAsync(addr, path, zip: a.Has("zip")),
                _ => await session.OppPush.SendFileAsync(addr, path, zip: a.Has("zip"))
            };
            if (ok) okCount++;
            else failed.Add(Path.GetFileName(path));
        }
        Info(existing.Count == okCount
            ? $"已推送 {okCount} 个文件"
            : $"推送完成：成功 {okCount}/{existing.Count}；失败：{string.Join("、", failed)}");
        return okCount == existing.Count ? 0 : 1;
    }

    public static async Task<int> DetectAsync(Args a)
    {
        var addr = a.Get(1);
        if (string.IsNullOrWhiteSpace(addr))
        {
            Error("用法：btcli detect <设备地址> [--db 路径]");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var device = await session.OppDiscovery.FindByAddressAsync(addr);
        if (device == null)
        {
            Error($"未找到设备：{addr}（请先扫描）");
            return 1;
        }
        var has = await AssistantDetector.DeviceHasAssistantAsync(device.Id);
        Info(has ? $"设备 {device.Name}（{device.AddrDisplay}）运行中：接收助手在线" : $"设备 {device.Name}（{device.AddrDisplay}）：未发现接收助手");
        return 0;
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
        var sub = (a.Get(1) ?? "").ToLowerInvariant();
        if (sub == "favorite")
        {
            var addr = a.Get(2);
            if (string.IsNullOrWhiteSpace(addr))
            {
                Error("用法：btcli devices favorite <设备地址> [--unset]");
                return 2;
            }
            var ok = session.Storage.SetDeviceFavorite(addr, !a.Has("unset"));
            Info(ok ? $"已更新收藏状态：{addr}" : $"设备不存在：{addr}");
            return ok ? 0 : 1;
        }
        if (sub == "alias")
        {
            var addr = a.Get(2);
            var alias = a.RemainingFrom(3);
            if (string.IsNullOrWhiteSpace(addr))
            {
                Error("用法：btcli devices alias <设备地址> <别名>（别名留空清除）");
                return 2;
            }
            var ok = session.Storage.SetDeviceAlias(addr, alias.Trim());
            Info(ok ? $"已更新别名：{addr} = \"{alias.Trim()}\"" : $"设备不存在：{addr}");
            return ok ? 0 : 1;
        }

        var sort = (a.Option("sort") ?? "last").ToLowerInvariant();
        var devices = session.Storage.GetDevices();
        IEnumerable<DeviceInfo> ordered;
        if (sort == "name")
            ordered = devices.OrderBy(d => d.Alias.Length > 0 ? d.Alias : d.Name);
        else if (sort == "favorite")
            ordered = devices.OrderByDescending(d => d.Favorite).ThenBy(d => d.Alias.Length > 0 ? d.Alias : d.Name);
        else
            ordered = devices.OrderByDescending(d => d.LastConnected);
        var list = ordered.ToList();

        if (a.Has("json"))
        {
            Console.WriteLine(JsonSerializer.Serialize(list.Select(d => new
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
        if (list.Count == 0)
        {
            Info("无已记录设备");
            return 0;
        }
        var header = new[] { "地址", "名称", "别名", "收藏", "最近连接" };
        var rows = list.Select(d => new[]
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
                Error($"未知配置项：{key}（可选 OppChunkSize/OppConnectTimeout/OppSendTimeout/PushTextFileName/OppAuthPassword/OppNameUseBom/OppProtectionLevel/TransferMode）");
                return 2;
            }
            session.Config.Save();
            Info($"已保存：{key} = {value}");
            return 0;
        }

        Info($"OPP 分块       OppChunkSize          = {session.Config.OppChunkSize}");
        Info($"OPP 连接超时    OppConnectTimeout     = {session.Config.OppConnectTimeoutSeconds} 秒");
        Info($"OPP 发送超时    OppSendTimeout        = {session.Config.OppSendTimeoutSeconds} 秒");
        Info($"OPP 文本文件名  PushTextFileName       = {session.Config.PushTextFileName}");
        Info($"OPP 认证密码    OppAuthPassword       = {(string.IsNullOrEmpty(session.Config.OppAuthPassword) ? "（未设置）" : "***")}");
        Info($"OPP Name BOM    OppNameUseBom         = {session.Config.OppNameUseBom}");
        Info($"OPP 保护级别    OppProtectionLevel    = {session.Config.OppProtectionLevel}（auto/plain/encrypt）");
        Info($"发送通道        TransferMode          = {session.Config.TransferMode}（auto/assistant/opp）");
        return 0;
    }

    private static bool SetConfigValue(AppConfig config, string key, string value)
    {
        switch (key.Trim().ToLowerInvariant())
        {
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
            case "transfermode":
                if (value is not ("auto" or "assistant" or "opp")) return false;
                config.TransferMode = value;
                return true;
            default:
                return false;
        }
    }

    // ------------------------------------------------------------------ 自检

    public static async Task<int> SelftestAsync(Args _)
    {
        var results = new List<(string Name, bool Ok, string Detail)>
        {
            TestStorageAndExport(),
            TestRetryPolicy(),
            TestZipPack(),
            TestDeviceManagement()
        };
        results.Add(await TestObexAsync());

        var failed = results.Count(r => !r.Ok);
        Console.WriteLine();
        Console.WriteLine("=== 自检结果 ===");
        foreach (var (name, ok, detail) in results)
            Console.WriteLine($"  [{(ok ? "PASS" : "FAIL")}] {name}{(detail.Length > 0 ? "：" + detail : "")}");
        Console.WriteLine(failed == 0 ? "全部通过 ✔" : $"{failed} 项失败 ✘");
        return failed == 0 ? 0 : 1;
    }

    private static (string Name, bool Ok, string Detail) TestRetryPolicy()
    {
        var ok = true;
        var policy = new OppRetryPolicy(3, 3);
        ok &= policy.MaxAttempts == 4;
        ok &= policy.ShouldRetry(1, "OBEX PUT 失败：Forbidden (0xC3)");
        ok &= policy.ShouldRetry(2, "对端提前关闭连接");
        ok &= !policy.ShouldRetry(4, "连接失败");
        ok &= !policy.ShouldRetry(1, "设备未配对");
        ok &= policy.NextDelay(2) == TimeSpan.FromSeconds(6);
        var tracker = new TransferSpeedTracker();
        tracker.AddSample(0);
        ok &= tracker.SpeedBytesPerSecond == 0;
        return ("重试策略 / 速率", ok,
            ok ? "错误分类/次数上限/退避/速率窗口全部通过" : "重试策略校验失败");
    }

    private static (string Name, bool Ok, string Detail) TestZipPack()
    {
        var dir = Path.Combine(Path.GetTempPath(), "btcli_zip_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(Path.Combine(dir, "sub"));
        try
        {
            File.WriteAllText(Path.Combine(dir, "a.txt"), "hello");
            File.WriteAllText(Path.Combine(dir, "sub", "b.txt"), "world");
            var zipPath = Path.Combine(dir, "out.zip");
            using (var archive = ZipFile.Open(zipPath, ZipArchiveMode.Create))
            {
                archive.CreateEntryFromFile(Path.Combine(dir, "a.txt"), "a.txt", CompressionLevel.Optimal);
                archive.CreateEntryFromFile(Path.Combine(dir, "sub", "b.txt"), "sub/b.txt", CompressionLevel.Optimal);
            }
            using var read = ZipFile.OpenRead(zipPath);
            var names = read.Entries.Select(e => e.FullName).OrderBy(x => x).ToList();
            var ok = names.SequenceEqual(new[] { "a.txt", "sub/b.txt" });
            return ("zip 打包", ok, ok ? "相对路径/子目录条目全部通过" : "zip 条目不符合预期");
        }
        catch (Exception ex)
        {
            return ("zip 打包", false, ex.Message);
        }
        finally
        {
            try
            {
                if (Directory.Exists(dir))
                    Directory.Delete(dir, recursive: true);
            }
            catch
            {
                // 清理失败不影响自检结论
            }
        }
    }

    private static (string Name, bool Ok, string Detail) TestDeviceManagement()
    {
        var dir = Path.Combine(Path.GetTempPath(), "btcli_dev_" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(dir);
        try
        {
            var storage = new StorageService(Path.Combine(dir, "dev.db"));
            storage.UpsertDevice(new DeviceInfo { Addr = "00:11:22:33:44:55", Name = "手机" });
            var ok = storage.SetDeviceFavorite("00:11:22:33:44:55", true);
            ok &= storage.SetDeviceAlias("00:11:22:33:44:55", "我的小米");
            var device = storage.GetDevices().Single();
            ok &= device.Favorite && device.Alias == "我的小米";
            ok &= !storage.SetDeviceFavorite("00:00:00:00:00:00", true);
            return ("设备管理", ok,
                ok ? "收藏/别名写入与读取全部通过" : "设备管理校验失败");
        }
        catch (Exception ex)
        {
            return ("设备管理", false, ex.Message);
        }
        finally
        {
            try
            {
                if (Directory.Exists(dir))
                    Directory.Delete(dir, recursive: true);
            }
            catch
            {
                // 清理失败不影响自检结论
            }
        }
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
            if (decoded == null || decoded.Length == 0 || longHeader.Value.Length > 250)
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
                        return 0;
                    case "clear":
                        Console.Clear();
                        break;
                    case "opp-scan":
                        await OppScanAsync(parts);
                        break;
                    case "opp-pair":
                        await OppPairAsync(parts);
                        break;
                    case "opp-send-file":
                        await OppSendFileAsync(parts);
                        break;
                    case "opp-send-text":
                        await OppSendTextAsync(parts);
                        break;
                    case "opp-send-folder":
                        await OppSendFolderAsync(parts);
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

    private static void ReplHelp()
    {
        Info("""
            help                    显示本帮助
            opp-scan [--seconds N] [--paired-only]
                                  扫描支持 OPP（蓝牙文件接收）的设备
            opp-pair <地址> [--pin 1234]
                                  发起配对
            opp-send-file <地址> <文件> [--zip]
                                  推送文件（可打包 zip）
            opp-send-text <地址> <文本> [--name 文件名]
                                  推送文本
            opp-send-folder <地址> <文件夹>
                                  推送文件夹（自动打包 zip）
            records / stats / devices / config / export <csv|json> <路径>
                                  查询记录、统计、设备、配置与导出
            quit / exit            退出
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
