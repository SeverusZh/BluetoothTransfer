using System.Text;

namespace BluetoothTransfer.Cli;

public static partial class Commands
{
    private static void Info(string message) => Console.WriteLine(message);
    private static void Warn(string message) => Console.Error.WriteLine($"[WARN] {message}");
    private static void Error(string message) => Console.Error.WriteLine($"[ERROR] {message}");

    private static CliSession NewSession(Args a, bool quiet = false, bool jsonEvents = false)
        => new(a.Option("db")) { Quiet = quiet && !a.Has("verbose"), JsonEvents = jsonEvents, Verbose = a.Has("verbose") };

    private static int ParseInt(string? text, int fallback)
        => int.TryParse(text, out var value) ? value : fallback;

    /// <summary>
    /// 统一选项校验：若 Args 存在缺值或未知选项错误，输出用法提示并返回 true（调用方应返回退出码 2）；
    /// 否则返回 false 表示可继续执行。
    /// </summary>
    private static bool RejectOnOptionError(Args a, string usage)
    {
        // 优先报告缺值错误，其次才是首个未知选项。
        var error = a.Error ?? (a.UnknownOptions.Count > 0 ? $"未知选项：--{a.UnknownOptions[0]}" : null);
        if (error == null) return false;
        Error($"用法：{usage}（{error}）");
        return true;
    }

    // ------------------------------------------------------------------ REPL

    public static async Task<int> ReplAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli repl"))
            return 2;
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

            var tokens = SplitArgs(line);
            // 会话指定了独立数据库时，让 records/stats/export 等子命令沿用同一数据库。
            if (replDb != null && !tokens.Any(t => t.StartsWith("--db", StringComparison.OrdinalIgnoreCase)))
            {
                tokens.Add("--db");
                tokens.Add(replDb);
            }
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

    /// <summary>
    /// 最小引号分词：用空白分隔 token，但双引号（"）包裹的内容整体保留为一个 token（内部空格不拆分）。
    /// 不要求转义嵌套；未闭合的引号按引号后内容整体处理。
    /// </summary>
    private static List<string> SplitArgs(string line)
    {
        var tokens = new List<string>();
        var current = new StringBuilder();
        var inQuote = false;
        foreach (var ch in line)
        {
            if (ch == '"')
            {
                inQuote = !inQuote;
                continue; // 引号本身不进入 token 内容
            }
            if (char.IsWhiteSpace(ch) && !inQuote)
            {
                if (current.Length > 0)
                {
                    tokens.Add(current.ToString());
                    current.Clear();
                }
                continue;
            }
            current.Append(ch);
        }
        if (current.Length > 0)
            tokens.Add(current.ToString());
        return tokens;
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
