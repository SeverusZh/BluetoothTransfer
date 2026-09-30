using System.Windows;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer;

/// <summary>
/// Interaction logic for App.xaml
/// </summary>
public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        base.OnStartup(e);
        // 不阻塞 UI 启动：后台恢复上次未完成的助手推送（durable send journal）。
        _ = Task.Run(ResumePendingSendsAsync);
    }

    /// <summary>
    /// 启动恢复：读 journal 中未完成的助手通道传输，按原文件名 + 原 SHA-256 重新发起，
    /// 复用既有 HELLO/OFFER(ResumeOffset) 续传协议从接收端已确认偏移继续。
    /// 说明：GUI 的 ViewModel 私有持有发送队列（本次不改 ViewModels），因此这里用独立服务实例直连重发；
    /// 同一 journal 的并发保护由租约（TransferJournal.Begin/TryClaim）保证，不会与 UI 手动重发双开。
    /// </summary>
    private static async Task ResumePendingSendsAsync()
    {
        try
        {
            var events = new EventBus();
            var config = AppConfig.Load();
            var storage = new StorageService();
            var discovery = new OppDiscoveryService(events);
            var packages = new PendingPackageStore();
            var journal = new TransferJournal(
                new FileTransferJournalStore(FileTransferJournalStore.DefaultPath), packages: packages);
            var push = new AssistantPushService(events, storage, config, discovery, journal, packages);
            var resume = new SendResumeService(journal, push, events, packages, (entry, reason) =>
                storage.AddRecord(new TransferRecord
                {
                    Direction = TransferConst.DirSend,
                    Type = TransferConst.TypeFile,
                    PeerName = entry.PeerName,
                    PeerAddr = entry.PeerAddr,
                    Name = entry.DisplayName,
                    Size = entry.Size,
                    Status = TransferConst.StatusFailed,
                    Channel = TransferConst.ChannelAssistant,
                    Note = $"断点续传已放弃：{reason}"
                }));
            await resume.ResumeAllAsync();
        }
        catch
        {
            // 启动恢复失败不影响应用启动；残留条目由 TTL（7 天）清理
        }
    }
}
