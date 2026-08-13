using System.IO;
using BluetoothTransfer.Core.Server;

namespace BluetoothTransfer.Receiver;

public static class ReceiverCli
{
    public static async Task<int> RunAsync(string[] args)
    {
        var command = (args.Length == 0 ? "help" : args[0]).ToLowerInvariant();
        switch (command)
        {
            case "help" or "--help" or "-h":
                PrintHelp();
                return 0;
            case "version" or "--version" or "-v":
                Console.WriteLine(typeof(ReceiverCli).Assembly.GetName().Version?.ToString(3) ?? "1.3.0");
                return 0;
            case "run":
                return await RunReceiverAsync(args[1..]);
            default:
                Console.Error.WriteLine($"[ERROR] 未知命令：{command}");
                PrintHelp();
                return 2;
        }
    }

    private static async Task<int> RunReceiverAsync(string[] args)
    {
        string? dir = null;
        // 安全加固：默认每次接收前询问（防止已配对设备未经确认直接落盘），--no-ask 显式关闭。
        var ask = true;
        for (var i = 0; i < args.Length; i++)
        {
            switch (args[i])
            {
                case "--dir" when i + 1 < args.Length:
                    dir = args[++i];
                    break;
                case "--ask":
                    ask = true;
                    break;
                case "--no-ask":
                    ask = false;
                    break;
                case "--help" or "-h":
                    PrintHelp();
                    return 0;
                default:
                    Console.Error.WriteLine($"[ERROR] 未知参数：{args[i]}");
                    return 2;
            }
        }

        var options = new ReceiverOptions
        {
            SaveDir = dir ?? Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
                "Downloads", "BluetoothReceive"),
            Ask = ask
        };

        await using var listener = await AsstRfcommService.StartAsync();
        var app = new ReceiverApp(options, listener,
            (level, msg) => Console.WriteLine($"[{DateTime.Now:HH:mm:ss}] [{level}] {msg}"),
            onProgress: (hello, sent, total) =>
            {
                var percent = total > 0 ? sent * 100.0 / total : 0.0;
                Console.WriteLine($"[{DateTime.Now:HH:mm:ss}] [PROGRESS] {hello.FileName}：{sent}/{total}（{percent:0.0}%）");
            },
            onCompleted: (hello, _) => Console.WriteLine($"[{DateTime.Now:HH:mm:ss}] [INFO] 已完成：{hello.FileName}"));

        using var cts = new CancellationTokenSource();
        Console.CancelKeyPress += (_, e) => { e.Cancel = true; cts.Cancel(); };
        try
        {
            await app.RunAsync(cts.Token);
        }
        catch (OperationCanceledException)
        {
            Console.WriteLine("已退出");
        }
        return 0;
    }

    private static void PrintHelp()
    {
        Console.WriteLine("""
            btrecv —— BluetoothTransfer 接收助手（Windows）

            用法：
              btrecv run [--dir 保存目录] [--no-ask]   启动接收（默认每次接收前询问）
              btrecv --help                           显示帮助
              btrecv --version                        显示版本

            说明：
              - 接收端与发送端需先配对；
              - 默认每次接收前询问，--no-ask 关闭询问、直接自动接收；
              - 助手必须保持运行才能接收，关闭即停止监听；
              - 传输为明文，接收完成时校验 SHA-256；
              - 半成品以 .btpart 保留，断线重连自动续传。
            """);
    }
}
