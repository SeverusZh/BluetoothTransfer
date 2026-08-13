using System.Text;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.Cli;

public static partial class Commands
{
    // ------------------------------------------------------------------ 发送模式 / 通道选择

    /// <summary>
    /// 解析 --mode 取值（缺省为 opp），并显式校验合法性。
    /// 非法值返回 null，由调用方输出用法错误并返回退出码 2。
    /// </summary>
    private static string? ResolveMode(Args a)
    {
        var mode = (a.Option("mode") ?? "opp").ToLowerInvariant();
        return mode is ("auto" or "assistant" or "opp") ? mode : null;
    }

    /// <summary>将内部通道标识（assistant/opp）转为面向用户的显示名。</summary>
    private static string ChannelName(string channel) => channel == "opp" ? "OPP" : "助手";

    /// <summary>
    /// 按发送模式选择通道：assistant 仅走助手通道；opp 仅走 OPP；
    /// auto 先尝试助手通道（返回 false 视为失败），失败时自动回退 OPP 并输出说明日志。
    /// 返回（是否成功，实际使用的通道）。
    /// </summary>
    private static async Task<(bool Ok, string Channel)> SendWithModeAsync(
        string mode, Func<Task<bool>> assistantTry, Func<Task<bool>> oppTry)
    {
        if (mode == "opp")
            return (await oppTry(), "opp");
        if (mode == "assistant")
            return (await assistantTry(), "assistant");
        // auto：助手在线优先，失败自动回退 OPP
        if (await assistantTry())
            return (true, "assistant");
        Info("助手通道失败/不可用，已回退 OPP");
        return (await oppTry(), "opp");
    }

    // ------------------------------------------------------------------ OPP 通用推送

    public static async Task<int> OppSendFileAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli opp-send-file <设备地址> <文件> [--zip] [--mode auto|assistant|opp] [--verbose] [--db 路径]"))
            return 2;
        var addr = a.Get(1);
        var path = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(path))
        {
            Error("用法：btcli opp-send-file <设备地址> <文件> [--zip] [--mode auto|assistant|opp] [--verbose] [--db 路径]");
            return 2;
        }
        if (!File.Exists(path))
        {
            Error($"文件不存在：{path}");
            return 2;
        }
        var mode = ResolveMode(a);
        if (mode == null)
        {
            Error("用法：btcli opp-send-file <设备地址> <文件> [--zip] [--mode auto|assistant|opp] [--verbose] [--db 路径]（--mode 仅支持 auto|assistant|opp）");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var (ok, channel) = await SendWithModeAsync(mode,
            () => session.AssistantPush.SendFileAsync(addr, path, zip: a.Has("zip")),
            () => session.OppPush.SendFileAsync(addr, path, zip: a.Has("zip")));
        Info(ok ? $"文件已推送（{ChannelName(channel)}）：{Path.GetFileName(path)}" : $"推送失败：{Path.GetFileName(path)}");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendTextAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli opp-send-text <设备地址> <文本> [--name 文件名] [--mode auto|assistant|opp] [--verbose] [--db 路径]"))
            return 2;
        var addr = a.Get(1);
        var text = a.RemainingFrom(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(text))
        {
            Error("用法：btcli opp-send-text <设备地址> <文本> [--name 文件名] [--mode auto|assistant|opp] [--verbose] [--db 路径]");
            return 2;
        }
        var mode = ResolveMode(a);
        if (mode == null)
        {
            Error("用法：btcli opp-send-text <设备地址> <文本> [--name 文件名] [--mode auto|assistant|opp] [--verbose] [--db 路径]（--mode 仅支持 auto|assistant|opp）");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var (ok, channel) = await SendWithModeAsync(mode,
            () => session.AssistantPush.SendTextAsync(addr, text, a.Option("name")),
            () => session.OppPush.SendTextAsync(addr, text, a.Option("name")));
        Info(ok
            ? $"文本已推送（{ChannelName(channel)}，{Encoding.UTF8.GetByteCount(text)} 字节）"
            : "文本推送失败");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendFolderAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli opp-send-folder <设备地址> <文件夹> [--mode auto|assistant|opp] [--verbose] [--db 路径]"))
            return 2;
        var addr = a.Get(1);
        var folder = a.Get(2);
        if (string.IsNullOrWhiteSpace(addr) || string.IsNullOrEmpty(folder))
        {
            Error("用法：btcli opp-send-folder <设备地址> <文件夹> [--mode auto|assistant|opp] [--verbose] [--db 路径]");
            return 2;
        }
        if (!Directory.Exists(folder))
        {
            Error($"文件夹不存在：{folder}");
            return 2;
        }
        var mode = ResolveMode(a);
        if (mode == null)
        {
            Error("用法：btcli opp-send-folder <设备地址> <文件夹> [--mode auto|assistant|opp] [--verbose] [--db 路径]（--mode 仅支持 auto|assistant|opp）");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var (ok, channel) = await SendWithModeAsync(mode,
            () => session.AssistantPush.SendFolderAsync(addr, folder),
            () => session.OppPush.SendFolderAsync(addr, folder));
        Info(ok ? $"文件夹已压缩并推送（{ChannelName(channel)}）：{folder}" : "文件夹推送失败");
        return ok ? 0 : 1;
    }

    public static async Task<int> OppSendFilesAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli opp-send-files <设备地址> <文件1> [文件2 ...] [--zip] [--mode auto|assistant|opp] [--verbose] [--db 路径]"))
            return 2;
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
            Error("用法：btcli opp-send-files <设备地址> <文件1> [文件2 ...] [--zip] [--verbose] [--db 路径]");
            return 2;
        }
        var existing = paths.Where(File.Exists).ToList();
        if (existing.Count == 0)
        {
            Error("没有可发送的文件（路径不存在）");
            return 2;
        }
        var mode = ResolveMode(a);
        if (mode == null)
        {
            Error("用法：btcli opp-send-files <设备地址> <文件1> [文件2 ...] [--zip] [--mode auto|assistant|opp] [--verbose] [--db 路径]（--mode 仅支持 auto|assistant|opp）");
            return 2;
        }
        using var session = NewSession(a, quiet: true);
        var okCount = 0;
        var failed = new List<string>();
        foreach (var path in existing)
        {
            var (ok, _) = await SendWithModeAsync(mode,
                () => session.AssistantPush.SendFileAsync(addr, path, zip: a.Has("zip")),
                () => session.OppPush.SendFileAsync(addr, path, zip: a.Has("zip")));
            if (ok) okCount++;
            else failed.Add(Path.GetFileName(path));
        }
        Info(existing.Count == okCount
            ? $"已推送 {okCount} 个文件"
            : $"推送完成：成功 {okCount}/{existing.Count}；失败：{string.Join("、", failed)}");
        return okCount == existing.Count ? 0 : 1;
    }
}
