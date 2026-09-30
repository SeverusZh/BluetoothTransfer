using System.IO;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Networking.Sockets;
using BluetoothTransfer.Core.Client;
using BluetoothTransfer.Core.Discovery;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>
/// 接收助手推送服务：与 OPP 相同的发送形态（文件/文本/文件夹），
/// 但走自定义 UUID 的私有分片协议，支持断点续传与 SHA-256 校验。
/// 记录写入 SQLite（channel=assistant）。
///
/// 跨进程/跨重启续传：每次发送前把“未完成传输”写入 durable send journal
/// （<see cref="TransferJournal"/>），成功后删除；zip/文本/文件夹的打包产物保留在
/// <see cref="PendingPackageStore"/>，在传输完成前不删除，保证重启后 SHA-256 与文件名可复现，
/// 从而命中接收端 <c>.btpart</c> 半成品并复用既有 HELLO/OFFER(ResumeOffset) 续传协议。
/// </summary>
public sealed class AssistantPushService : IAssistantPushService, IJournalEntrySender
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;
    private readonly TransferJournal _journal;
    private readonly PendingPackageStore _packages;
    private int _maintenanceDone;

    public AssistantPushService(
        EventBus events,
        StorageService storage,
        AppConfig config,
        OppDiscoveryService? discovery = null,
        TransferJournal? journal = null,
        PendingPackageStore? packages = null)
    {
        _events = events ?? throw new ArgumentNullException(nameof(events));
        _storage = storage ?? throw new ArgumentNullException(nameof(storage));
        _config = config ?? throw new ArgumentNullException(nameof(config));
        _discovery = discovery ?? new OppDiscoveryService(events);
        _packages = packages ?? new PendingPackageStore();
        // 默认 journal 以本服务的产物目录作为清理器，删除条目时同步删除打包产物。
        _journal = journal ?? new TransferJournal(
            new FileTransferJournalStore(FileTransferJournalStore.DefaultPath), packages: _packages);
    }

    public async Task<bool> SendFileAsync(string deviceAddr, string filePath, bool zip = false, CancellationToken ct = default,
        Func<bool>? cancelledByPause = null)
    {
        if (string.IsNullOrWhiteSpace(filePath) || !File.Exists(filePath))
        {
            _events.Publish(new LogEvent("ERROR", $"文件不存在：{filePath}"));
            return false;
        }
        var name = new FileInfo(filePath).Name;
        if (zip)
        {
            // 打包产物保留在待续传目录（不随本次发送删除），保证重启后 SHA 可复现。
            string payload;
            try
            {
                payload = await _packages.CreateZipFromFileAsync(filePath, ct);
            }
            catch (Exception ex)
            {
                _events.Publish(new LogEvent("ERROR", $"文件打包失败：{ex.Message}"));
                return false;
            }
            return await SendPreparedAsync(deviceAddr, payload, name + ".zip", filePath,
                TransferJournalKind.FileZip, producedPackage: true, sourcePath: filePath, cancelledByPause, ct);
        }
        return await SendPreparedAsync(deviceAddr, filePath, name, filePath,
            TransferJournalKind.File, producedPackage: false, sourcePath: filePath, cancelledByPause, ct);
    }

    public async Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default,
        Func<bool>? cancelledByPause = null)
    {
        var fileName = string.IsNullOrWhiteSpace(name) ? _config.PushTextFileName : name;
        string payload;
        try
        {
            payload = await _packages.CreateTextFileAsync(text ?? "", fileName, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"文本临时文件写入失败：{ex.Message}"));
            return false;
        }
        return await SendPreparedAsync(deviceAddr, payload, fileName, payload,
            TransferJournalKind.Text, producedPackage: true, sourcePath: "", cancelledByPause, ct);
    }

    public async Task<bool> SendFolderAsync(string deviceAddr, string folderPath, CancellationToken ct = default,
        Func<bool>? cancelledByPause = null)
    {
        if (string.IsNullOrWhiteSpace(folderPath) || !Directory.Exists(folderPath))
        {
            _events.Publish(new LogEvent("ERROR", $"文件夹不存在：{folderPath}"));
            return false;
        }
        var folderName = Path.GetFileName(folderPath.TrimEnd('\\', '/'));
        if (string.IsNullOrEmpty(folderName)) folderName = "folder";
        string payload;
        try
        {
            payload = await _packages.CreateZipFromFolderAsync(folderPath, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"文件夹打包失败：{ex.Message}"));
            return false;
        }
        return await SendPreparedAsync(deviceAddr, payload, folderName + ".zip", folderPath,
            TransferJournalKind.Folder, producedPackage: true, sourcePath: folderPath, cancelledByPause, ct);
    }

    /// <summary>
    /// 按 journal 条目重新发送（启动恢复路径）。不写记录、不改条目状态——条目生命周期由
    /// <see cref="SendResumeService"/> 统一管理；这里只做“用同一份字节与同一个文件名重新走 OFFER 续传”。
    /// </summary>
    public async Task<JournalSendOutcome> SendJournalEntryAsync(TransferJournalEntry entry, CancellationToken ct = default)
    {
        ArgumentNullException.ThrowIfNull(entry);
        var (device, error) = await ResolveDeviceAsync(entry.PeerAddr, ct);
        if (device == null) return JournalSendOutcome.Failure(error);
        if (!device.IsPaired) return JournalSendOutcome.Failure("设备未配对");
        return await RunTransferAsync(device, entry, ct);
    }

    /// <summary>打包 → 校验和 → 写 journal（取得发送租约）→ OFFER 续传 → 按结果收敛 journal 与打包产物。</summary>
    private async Task<bool> SendPreparedAsync(
        string deviceAddr,
        string payloadPath,
        string displayName,
        string localPathForRecord,
        string kind,
        bool producedPackage,
        string sourcePath,
        Func<bool>? cancelledByPause,
        CancellationToken ct)
    {
        EnsureMaintenance();

        var (device, resolveError) = await ResolveDeviceAsync(deviceAddr, ct);
        if (device == null)
        {
            DeleteProduced(payloadPath, producedPackage);
            WriteFailedRecord("", deviceAddr, displayName, 0, resolveError);
            return false;
        }
        if (!device.IsPaired)
        {
            var message = $"设备未配对：{device.Name}（{device.AddrDisplay}），请先在应用内或系统设置中完成配对";
            _events.Publish(new LogEvent("ERROR", message));
            DeleteProduced(payloadPath, producedPackage);
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, "设备未配对");
            return false;
        }

        long totalLength;
        string checksum;
        try
        {
            totalLength = new FileInfo(payloadPath).Length;
            checksum = await FileChecksum.ComputeSha256Async(payloadPath);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"计算校验和失败：{ex.Message}"));
            DeleteProduced(payloadPath, producedPackage);
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, ex.Message);
            return false;
        }

        // 用内容 SHA-256 作为 transferId：同一文件重发/重试/重启恢复都能命中接收端半成品。
        var entry = new TransferJournalEntry
        {
            Id = Guid.NewGuid().ToString("N"),
            PeerAddr = device.Addr,
            PeerName = device.Name,
            DisplayName = displayName,
            Size = totalLength,
            Sha256 = checksum,
            PayloadPath = payloadPath,
            SourcePath = sourcePath,
            Kind = kind,
            IsProducedPackage = producedPackage,
            State = TransferJournalState.Pending,
            CreatedAt = DateTime.UtcNow.ToString("o")
        };

        JournalBeginResult begin;
        try
        {
            begin = _journal.Begin(entry);
        }
        catch (Exception ex)
        {
            // journal 不可写不阻塞本次发送（代价：进程结束后无法自动恢复）。
            _events.Publish(new LogEvent("WARN", $"写入发送日志失败（本次仍会发送，重启后无法自动续传）：{ex.Message}"));
            begin = new JournalBeginResult(entry, Reused: false, Busy: false);
        }

        if (begin.Busy)
        {
            // 同一 (对端, 文件名, 内容 SHA) 已被另一次运行按有效租约占用：拒绝并发双开。
            if (producedPackage && !string.Equals(payloadPath, begin.Entry.PayloadPath, StringComparison.OrdinalIgnoreCase))
                _packages.Delete(payloadPath);
            _events.Publish(new LogEvent("WARN", $"同一传输正在发送/恢复中，已跳过重复发起：{displayName}"));
            return false;
        }

        var active = begin.Entry;
        var outcome = await RunTransferAsync(device, active, ct);

        if (outcome.Ok)
        {
            SafeJournal(() => _journal.Complete(active.Id));
            DeleteProduced(active.PayloadPath, active.IsProducedPackage);
            var resumeNote = outcome.ResumeOffset > 0 ? $"（从偏移 {outcome.ResumeOffset} 续传）" : "";
            _events.Publish(new LogEvent("INFO",
                $"助手推送成功：{displayName} -> {device.Name}（{device.AddrDisplay}，{outcome.BytesSent} 字节{resumeNote}）"));
            WriteOkRecord(device, displayName, totalLength, checksum, localPathForRecord, outcome.BytesSent, resumeNote);
            return true;
        }

        if (outcome.Cancelled)
        {
            if (IsPaused(cancelledByPause))
            {
                // 用户暂停：保留条目与打包产物，但不允许启动恢复自动重发（等用户点“继续”）。
                SafeJournal(() => _journal.Pause(active.Id));
                _events.Publish(new LogEvent("WARN", $"助手推送已暂停（保留断点与打包产物）：{displayName}"));
            }
            else
            {
                // 用户取消（或取消全部）：清理 journal 与打包产物。
                SafeJournal(() => _journal.Remove(active.Id));
                DeleteProduced(active.PayloadPath, active.IsProducedPackage);
                WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消");
                _events.Publish(new LogEvent("WARN", $"助手推送已取消：{displayName}"));
            }
            return false;
        }

        // 可恢复失败（对端不可达/超时/被中断）：保留条目与打包产物，下次启动或重试从偏移续传。
        SafeJournal(() => _journal.Release(active.Id, outcome.Error));
        _events.Publish(new LogEvent("ERROR", $"助手推送失败：{displayName}（{outcome.Error}）"));
        WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, outcome.Error);
        return false;
    }

    /// <summary>
    /// 传输主体：HELLO/OFFER → DATA/ACK → DONE，含进程内有界重试（OppRetryPolicy）。
    /// 与 journal 状态无关，只负责“把这份字节送出去”。
    /// </summary>
    private async Task<JournalSendOutcome> RunTransferAsync(OppDeviceInfo device, TransferJournalEntry entry, CancellationToken ct)
    {
        var retryPolicy = new OppRetryPolicy(_config.OppRetryCount, _config.OppRetryDelaySeconds);
        var renewal = new LeaseRenewal(_journal, entry.Id);
        var attempt = 1;
        var lastError = "";
        while (true)
        {
            RfcommDeviceService? service = null;
            try
            {
                service = await AssistantDetector.GetAssistantServiceAsync(device.Id, ct);
                if (service == null)
                    throw new AsstProtocolException("对端未运行接收助手（探测不到助手服务）");
                var level = ResolveProtectionLevel(service, _config.OppProtectionLevel);
                var transport = await SocketAsstTransport.ConnectAsync(service, level, ct);
                await using var client = new AssistantClient(transport);
                using var fs = new FileStream(entry.PayloadPath, FileMode.Open, FileAccess.Read, FileShare.Read);
                var hello = new AsstHello(entry.Sha256, entry.DisplayName, entry.Size, _config.OppChunkSize, entry.Sha256);
                var result = await client.SendAsync(hello, (offset, buffer, token) =>
                {
                    fs.Seek(offset, SeekOrigin.Begin);
                    return fs.ReadAsync(buffer, token).AsTask();
                }, (sent, total) =>
                {
                    _events.Publish(new TransferProgressEvent(entry.Sha256, sent, total, 0));
                    renewal.Tick();
                }, ct);

                if (!result.Ok)
                {
                    if (result.Error.Contains("忙"))
                    {
                        lastError = "接收端忙（连接重试）";
                        if (retryPolicy.ShouldRetry(attempt, lastError))
                        {
                            var delay = retryPolicy.NextDelay(attempt);
                            _events.Publish(new LogEvent("WARN",
                                $"接收端忙（{entry.DisplayName}）：{delay.TotalSeconds:0} 秒后第 {attempt + 1}/{retryPolicy.MaxAttempts} 次重试"));
                            attempt++;
                            if (!await TryDelayAsync(delay, ct)) return JournalSendOutcome.CancelledOutcome();
                            continue;
                        }
                        return JournalSendOutcome.Failure($"接收端忙（已重试 {attempt - 1} 次）");
                    }
                    return JournalSendOutcome.Failure(result.Error);
                }

                _events.Publish(new TransferProgressEvent(entry.Sha256, entry.Size, entry.Size, 0));
                _storage.UpsertDevice(new DeviceInfo
                {
                    Addr = device.Addr,
                    Name = device.Name,
                    LastSeen = DateTime.Now.ToString("o"),
                    LastConnected = DateTime.Now.ToString("o")
                });
                return JournalSendOutcome.Success(result.BytesSent, result.ResumeOffset);
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                return JournalSendOutcome.CancelledOutcome();
            }
            catch (Exception ex)
            {
                lastError = string.IsNullOrWhiteSpace(ex.Message)
                    ? $"{ex.GetType().Name} (0x{ex.HResult:X8})"
                    : ex.Message;
            }
            finally
            {
                service?.Dispose();
            }

            if (retryPolicy.ShouldRetry(attempt, lastError))
            {
                var delay = retryPolicy.NextDelay(attempt);
                _events.Publish(new LogEvent("WARN",
                    $"助手推送失败（{entry.DisplayName}）：{lastError}；{delay.TotalSeconds:0} 秒后第 {attempt + 1}/{retryPolicy.MaxAttempts} 次重试（将从偏移续传）"));
                attempt++;
                if (!await TryDelayAsync(delay, ct)) return JournalSendOutcome.CancelledOutcome();
                continue;
            }

            var note = attempt > 1 ? $"{lastError}（已重试 {attempt - 1} 次）" : lastError;
            return JournalSendOutcome.Failure(note);
        }
    }

    private async Task<(OppDeviceInfo? Device, string Error)> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
    {
        OppDeviceInfo? device = null;
        try
        {
            device = await _discovery.FindByAddressAsync(deviceAddr, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("WARN", $"OPP 设备发现失败（尝试按已配对设备解析）：{ex.Message}"));
        }
        if (device != null) return (device, "");

        // 接收端只运行助手（不广播 OPP）时，OPP 发现找不到设备，回退到已配对设备列表
        var paired = await AssistantDetector.FindPairedDeviceAsync(deviceAddr, ct);
        if (paired == null)
        {
            var message = $"未找到蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）";
            _events.Publish(new LogEvent("ERROR", message));
            return (null, message);
        }
        return (new OppDeviceInfo
        {
            Id = paired.DeviceId,
            Addr = paired.Addr,
            Name = paired.Name,
            IsPaired = true
        }, "");
    }

    /// <summary>每个进程只做一次 journal 维护：TTL/上限清理 + 孤儿打包产物清理（CLI 场景同样收敛）。</summary>
    private void EnsureMaintenance()
    {
        if (Interlocked.Exchange(ref _maintenanceDone, 1) == 1) return;
        try
        {
            _journal.CleanupExpired();
            _packages.CleanOrphans(_journal.ReadAll().Select(e => e.PayloadPath), TransferJournal.DefaultTtl);
        }
        catch
        {
            // 维护失败不影响发送
        }
    }

    private static async Task<bool> TryDelayAsync(TimeSpan delay, CancellationToken ct)
    {
        try
        {
            await Task.Delay(delay, ct);
            return true;
        }
        catch (OperationCanceledException)
        {
            return false;
        }
    }

    private static bool IsPaused(Func<bool>? cancelledByPause)
    {
        try { return cancelledByPause?.Invoke() == true; }
        catch { return false; }
    }

    private static void SafeJournal(Action action)
    {
        try { action(); }
        catch { /* journal 写入失败不改变本次发送结果 */ }
    }

    private void DeleteProduced(string payloadPath, bool producedPackage)
    {
        if (!producedPackage) return;
        _packages.Delete(payloadPath);
    }

    private void WriteOkRecord(OppDeviceInfo device, string displayName, long size, string checksum, string localPath, long bytesSent, string resumeNote = "")
    {
        PushSendHelper.WriteOkRecord(_storage, TransferConst.ChannelAssistant,
            device.Name, device.Addr, displayName, size, checksum, localPath, bytesSent, resumeNote);
    }

    private void WriteFailedRecord(string peerName, string peerAddr, string displayName, long size, string note)
    {
        PushSendHelper.WriteFailedRecord(_storage, TransferConst.ChannelAssistant, peerName, peerAddr, displayName, size, note);
    }

    /// <summary>
    /// 解析助手通道的连接保护级别，与 OPP 通道共用 <see cref="AppConfig.OppProtectionLevel"/> 配置。
    /// auto：按服务端 SDP 要求的保护级别连接（服务未要求加密则保持明文，与 OPP 路径一致）；
    /// plain：强制明文；encrypt：强制加密认证。
    /// </summary>
    private static SocketProtectionLevel ResolveProtectionLevel(RfcommDeviceService service, string config)
    {
        switch (config?.Trim().ToLowerInvariant())
        {
            case "plain":
                return SocketProtectionLevel.PlainSocket;
            case "encrypt":
                return SocketProtectionLevel.BluetoothEncryptionWithAuthentication;
            default: // auto
                return service.ProtectionLevel != SocketProtectionLevel.PlainSocket
                    ? service.ProtectionLevel
                    : SocketProtectionLevel.PlainSocket;
        }
    }

    /// <summary>按固定间隔为 journal 条目续租，避免长传输期间租约过期被启动恢复误判为崩溃残留。</summary>
    private sealed class LeaseRenewal
    {
        private readonly TransferJournal _journal;
        private readonly string _entryId;
        private DateTime _lastRenewUtc = DateTime.MinValue;

        public LeaseRenewal(TransferJournal journal, string entryId)
        {
            _journal = journal;
            _entryId = entryId;
        }

        public void Tick()
        {
            var now = DateTime.UtcNow;
            if (now - _lastRenewUtc < TransferJournal.RenewInterval) return;
            _lastRenewUtc = now;
            try { _journal.Renew(_entryId); }
            catch { /* 续租失败不影响传输 */ }
        }
    }
}
