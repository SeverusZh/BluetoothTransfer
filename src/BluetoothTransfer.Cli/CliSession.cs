using System.Text.Json;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.Cli;

/// <summary>
/// CLI 会话：与 GUI 采用相同的服务装配方式（无 WPF 依赖），
/// 事件通过控制台输出，方便人工观察与脚本解析。
/// </summary>
public sealed class CliSession : IDisposable
{
    public EventBus Events { get; } = new();
    public StorageService Storage { get; }
    public AppConfig Config { get; }
    public OppDiscoveryService OppDiscovery { get; }
    public OppPushService OppPush { get; }
    public AssistantPushService AssistantPush { get; }

    /// <summary>只输出关键结果，不打印 INFO/DEBUG 日志与进度条。</summary>
    public bool Quiet { get; init; }

    /// <summary>事件以 JSON Lines 输出，便于测试脚本解析。</summary>
    public bool JsonEvents { get; init; }

    /// <summary>打印 DEBUG 级别日志（默认过滤）。</summary>
    public bool Verbose { get; init; }

    public CliSession(string? dbPath = null)
    {
        Storage = new StorageService(dbPath);
        Config = AppConfig.Load();
        OppDiscovery = new OppDiscoveryService(Events);
        OppPush = new OppPushService(Events, Storage, Config, OppDiscovery);
        AssistantPush = new AssistantPushService(Events, Storage, Config, OppDiscovery);
        SubscribeEvents();
    }

    private void SubscribeEvents()
    {
        Events.Subscribe<LogEvent>(OnLog);
        Events.Subscribe<TransferProgressEvent>(OnTransferProgress);
    }

    private void OnLog(LogEvent e)
    {
        if (Quiet) return;
        if (e.Level == "DEBUG" && !Verbose) return;

        if (JsonEvents)
        {
            PrintJson(new { @event = "log", level = e.Level, message = e.Message });
            return;
        }

        var color = e.Level switch
        {
            "ERROR" => ConsoleColor.Red,
            "WARN" => ConsoleColor.Yellow,
            _ => ConsoleColor.Gray
        };
        var old = Console.ForegroundColor;
        Console.ForegroundColor = color;
        Console.WriteLine($"[{DateTime.Now:HH:mm:ss}] [{e.Level}] {e.Message}");
        Console.ForegroundColor = old;
    }

    private void OnTransferProgress(TransferProgressEvent e)
    {
        if (Quiet || JsonEvents || Console.IsErrorRedirected) return;
        var percent = e.TotalBytes > 0 ? e.BytesSent * 100.0 / e.TotalBytes : 0.0;
        Console.Error.Write($"\r任务 {e.TaskId}：{percent,6:0.0}%  {e.BytesSent}/{e.TotalBytes} 字节   ");
        if (e.BytesSent >= e.TotalBytes) Console.Error.WriteLine();
    }

    public void Dispose()
    {
    }

    private static void PrintJson(object value)
        => Console.WriteLine(JsonSerializer.Serialize(value));
}
