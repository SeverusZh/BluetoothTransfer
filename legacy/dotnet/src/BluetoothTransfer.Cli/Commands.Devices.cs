using System.Diagnostics;
using System.Text.Json;
using BluetoothTransfer.Core.Discovery;

namespace BluetoothTransfer.Cli;

public static partial class Commands
{
    // ------------------------------------------------------------------ OPP 通用推送

    public static async Task<int> OppScanAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli opp-scan [--seconds 秒] [--paired-only] [--json] [--db 路径]"))
            return 2;
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
        if (RejectOnOptionError(a, "btcli opp-pair <设备地址> [--pin 1234] [--db 路径]"))
            return 2;
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

    public static async Task<int> DetectAsync(Args a)
    {
        if (RejectOnOptionError(a, "btcli detect <设备地址> [--db 路径]"))
            return 2;
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

    public static int PackageReceiver(Args a)
    {
        if (RejectOnOptionError(a, "btcli package-receiver [--root 仓库根]"))
            return 2;
        var root = a.Option("root");
        if (string.IsNullOrEmpty(root))
        {
            var dir = new DirectoryInfo(AppContext.BaseDirectory);
            while (dir != null && !File.Exists(Path.Combine(dir.FullName, "BluetoothTransfer.sln")))
                dir = dir.Parent;
            root = dir?.FullName ?? "";
        }
        var script = Path.Combine(root, "tools", "publish-receiver.ps1");
        if (!File.Exists(script))
        {
            Error($"找不到发布脚本：{script}（可用 --root 指定仓库根目录）");
            return 2;
        }
        var psi = new ProcessStartInfo("powershell",
            $"-NoProfile -ExecutionPolicy Bypass -File \"{script}\"")
        {
            UseShellExecute = false
        };
        using var proc = Process.Start(psi);
        proc?.WaitForExit();
        return proc?.ExitCode == 0 ? 0 : 1;
    }
}
