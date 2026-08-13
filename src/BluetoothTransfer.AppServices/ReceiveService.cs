using System.IO;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>
/// 完整 GUI 内置接收助手：复用 Core 的 RFCOMM 服务端/文件落盘/断点续传，
/// 接收完成写入 SQLite（direction=recv, channel=assistant）。
/// 测试可注入 MemoryAsstListener。
/// </summary>
public sealed class ReceiveService : IReceiveService
{
    private readonly StorageService _storage;
    private readonly EventBus _events;
    private readonly IAsstListener? _listenerOverride;
    private AsstRfcommService? _realListener;
    private AssistantServer? _server;
    private CancellationTokenSource? _cts;
    private Task? _runTask;

    public ReceiveService(StorageService storage, EventBus events, IAsstListener? listenerOverride = null)
    {
        _storage = storage ?? throw new ArgumentNullException(nameof(storage));
        _events = events ?? throw new ArgumentNullException(nameof(events));
        _listenerOverride = listenerOverride;
    }

    public bool IsListening { get; private set; }

    /// <summary>询问处理器：由 UI 弹出确认；返回 true 接受。</summary>
    public Func<AsstHello, Task<bool>>? AskHandler { get; set; }

    public event Action<string, long, long>? ProgressChanged;
    public event Action<string>? Completed;
    public event Action<string, string>? Logged;

    public async Task StartAsync(string saveDir, bool ask, CancellationToken ct = default)
    {
        if (IsListening) return;
        _cts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        Directory.CreateDirectory(saveDir);
        AsstFileSink.CleanStale(saveDir, TimeSpan.FromDays(7));
        var sink = new AsstFileSink(saveDir);
        IAsstListener listener;
        if (_listenerOverride != null)
        {
            listener = _listenerOverride;
        }
        else
        {
            _realListener = await AsstRfcommService.StartAsync();
            listener = _realListener;
        }
        _server = new AssistantServer(listener, sink,
            approval: ask ? AskAsync : null,
            onLog: (level, msg) => Logged?.Invoke(level, msg),
            onProgress: (hello, sent, total) => ProgressChanged?.Invoke(hello.FileName, sent, total),
            onCompleted: (hello, peerAddr) =>
            {
                WriteReceiveRecord(hello, peerAddr);
                Completed?.Invoke(hello.FileName);
            });
        IsListening = true;
        _runTask = Task.Run(() => _server.RunAsync(_cts.Token));
        _events.Publish(new LogEvent("INFO", $"接收助手已开始监听（目录：{saveDir}）"));
    }

    public async Task StopAsync()
    {
        if (!IsListening) return;
        IsListening = false;
        _cts?.Cancel();
        var server = _server;
        var listener = _realListener;
        _server = null;
        _realListener = null;
        try
        {
            if (server != null) await server.DisposeAsync().AsTask();
            if (listener != null) await listener.DisposeAsync().AsTask();
            if (_runTask != null) await _runTask;
        }
        catch (OperationCanceledException)
        {
            // 正常停止
        }
        finally
        {
            _cts?.Dispose();
            _cts = null;
            _runTask = null;
        }
        _events.Publish(new LogEvent("INFO", "接收助手已停止监听"));
    }

    private async Task<AsstOfferStatus> AskAsync(AsstHello hello)
    {
        var ok = AskHandler != null && await AskHandler(hello);
        return ok ? AsstOfferStatus.Accept : AsstOfferStatus.Reject;
    }

    private void WriteReceiveRecord(AsstHello hello, string? peerAddr)
    {
        try
        {
            var isText = string.Equals(Path.GetExtension(hello.FileName), ".txt",
                StringComparison.OrdinalIgnoreCase);
            var normalizedAddr = NormalizeAddr(peerAddr);
            var peerName = "";
            if (!string.IsNullOrEmpty(normalizedAddr))
            {
                var device = _storage.GetDevices()
                    .FirstOrDefault(d => OppDiscoveryService.AddrEquals(d.Addr, normalizedAddr));
                peerName = device?.Name ?? "";
            }
            _storage.AddRecord(new TransferRecord
            {
                Direction = TransferConst.DirRecv,
                Type = isText ? TransferConst.TypeText : TransferConst.TypeFile,
                PeerName = peerName,
                PeerAddr = normalizedAddr,
                Name = hello.FileName,
                Size = hello.FileSize,
                Status = TransferConst.StatusOk,
                Checksum = hello.ExpectedSha256,
                Channel = TransferConst.ChannelAssistant,
                Note = "接收助手"
            });
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"接收记录写入失败：{ex.Message}"));
        }
    }

    /// <summary>把对端地址规范化为 aa:bb:cc:dd:ee:ff；无法解析时返回原值。</summary>
    private static string NormalizeAddr(string? addr)
    {
        if (string.IsNullOrWhiteSpace(addr)) return "";
        var hex = new string(addr.Where(c => Uri.IsHexDigit(c)).ToArray()).ToLowerInvariant();
        if (hex.Length != 12) return addr;
        return string.Join(":", Enumerable.Range(0, 6).Select(i => hex.Substring(i * 2, 2)));
    }
}
