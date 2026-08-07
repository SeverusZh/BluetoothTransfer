using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;

namespace BluetoothTransfer.Receiver;

/// <summary>接收助手 CLI 内核：监听 + 落盘 + 可选询问。真实与内存监听器均可注入。</summary>
public sealed class ReceiverApp
{
    private readonly ReceiverOptions _options;
    private readonly AssistantServer _server;

    public ReceiverApp(
        ReceiverOptions options,
        IAsstListener listener,
        Action<string, string>? onLog = null,
        Action<AsstHello, long, long>? onProgress = null,
        Action<AsstHello, string?>? onCompleted = null)
    {
        _options = options ?? throw new ArgumentNullException(nameof(options));
        if (listener == null) throw new ArgumentNullException(nameof(listener));
        var sink = new AsstFileSink(options.SaveDir);
        _server = new AssistantServer(listener, sink, options.Ask ? AskAsync : null, onLog, onProgress, onCompleted);
        AsstFileSink.CleanStale(options.SaveDir, TimeSpan.FromDays(7));
    }

    private async Task<AsstOfferStatus> AskAsync(AsstHello hello)
    {
        Console.Write($"接收文件 {hello.FileName}（{hello.FileSize:N0} 字节）？[y/N] ");
        var line = await Task.Run(() => Console.ReadLine());
        return string.Equals(line?.Trim(), "y", StringComparison.OrdinalIgnoreCase)
            ? AsstOfferStatus.Accept
            : AsstOfferStatus.Reject;
    }

    public async Task RunAsync(CancellationToken ct = default)
    {
        Console.WriteLine($"接收目录：{_options.SaveDir}");
        Console.WriteLine($"模式：{(_options.Ask ? "每次询问" : "自动接收")}");
        Console.WriteLine("接收助手已启动，等待连接…（Ctrl+C 退出）");
        await _server.RunAsync(ct);
    }
}
