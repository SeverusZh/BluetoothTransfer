namespace BluetoothTransfer.Services;

/// <summary>一次（恢复）发送的结果：成功 / 用户或系统取消 / 可重试失败。</summary>
public sealed record JournalSendOutcome(bool Ok, bool Cancelled, string Error, long BytesSent = 0, long ResumeOffset = 0)
{
    public static JournalSendOutcome Success(long bytesSent, long resumeOffset)
        => new(true, false, "", bytesSent, resumeOffset);

    public static JournalSendOutcome Failure(string error)
        => new(false, false, string.IsNullOrWhiteSpace(error) ? "未知错误" : error);

    public static JournalSendOutcome CancelledOutcome(string reason = "用户取消")
        => new(false, true, reason);
}

/// <summary>
/// 按 journal 条目重新发起一次助手推送的传输执行者（由 <see cref="AssistantPushService"/> 实现）。
/// 抽成接口是为了让恢复编排可在无蓝牙/无 Windows 的环境用 fake 做真实回归测试。
/// </summary>
public interface IJournalEntrySender
{
    Task<JournalSendOutcome> SendJournalEntryAsync(TransferJournalEntry entry, CancellationToken ct = default);
}

/// <summary>
/// 启动恢复编排：把 journal 中“未完成且可恢复”的传输重新发起，走既有 HELLO/OFFER 续传协议
/// （不新增第二套续传协议）。按对端串行，避免同一设备并发连接。
/// </summary>
public sealed class SendResumeService
{
    private readonly TransferJournal _journal;
    private readonly IJournalEntrySender _sender;
    private readonly EventBus? _events;
    private readonly PendingPackageStore? _packages;
    private readonly Action<TransferJournalEntry, string>? _onAbandoned;

    public SendResumeService(
        TransferJournal journal,
        IJournalEntrySender sender,
        EventBus? events = null,
        PendingPackageStore? packages = null,
        Action<TransferJournalEntry, string>? onAbandoned = null)
    {
        _journal = journal ?? throw new ArgumentNullException(nameof(journal));
        _sender = sender ?? throw new ArgumentNullException(nameof(sender));
        _events = events;
        _packages = packages;
        _onAbandoned = onAbandoned;
    }

    /// <summary>
    /// 恢复全部可续传条目，返回本次尝试发送的条数。
    /// 幂等：每次恢复都先 TryClaim（租约/幂等键）取得独占权，重复调用不会双开同一条目。
    /// </summary>
    public async Task<int> ResumeAllAsync(CancellationToken ct = default)
    {
        // 启动即收敛：清 TTL/超限条目与其产物，并清理无主孤儿产物。
        _journal.CleanupExpired();
        CleanOrphanPackages();

        var plan = _journal.PlanRecovery();
        foreach (var item in plan.Abandoned)
        {
            _events?.Publish(new LogEvent("WARN", $"放弃未完成传输：{item.Entry.DisplayName}（{item.Reason}）"));
            try { _onAbandoned?.Invoke(item.Entry, item.Reason); } catch { /* 记录失败不影响恢复 */ }
        }
        if (plan.Abandoned.Count > 0) _journal.ApplyAbandoned(plan.Abandoned);

        var attempted = 0;
        foreach (var group in plan.Recoverable.GroupBy(e => e.PeerAddr ?? "", StringComparer.OrdinalIgnoreCase))
        {
            foreach (var entry in group)
            {
                ct.ThrowIfCancellationRequested();
                if (!_journal.TryClaim(entry.Id)) continue; // 已被其他运行占用 / 已暂停 / 已删除
                attempted++;

                JournalSendOutcome outcome;
                try
                {
                    outcome = await _sender.SendJournalEntryAsync(entry, ct);
                }
                catch (OperationCanceledException)
                {
                    // 应用退出导致的取消：保留条目，下次启动继续（与“用户取消”语义区分）。
                    _journal.Release(entry.Id, "恢复被取消，保留待下次启动", countAttempt: false);
                    throw;
                }
                catch (Exception ex)
                {
                    outcome = JournalSendOutcome.Failure(ex.Message);
                }

                if (outcome.Ok)
                {
                    _journal.Complete(entry.Id);
                    _events?.Publish(new LogEvent("INFO",
                        $"断点续传恢复成功：{entry.DisplayName} -> {entry.PeerAddr}（{outcome.BytesSent} 字节，起始偏移 {outcome.ResumeOffset}）"));
                }
                else if (outcome.Cancelled)
                {
                    _journal.Release(entry.Id, "恢复被取消，保留待下次启动", countAttempt: false);
                }
                else
                {
                    _journal.Release(entry.Id, outcome.Error);
                    _events?.Publish(new LogEvent("WARN",
                        $"恢复未完成，已保留待下次：{entry.DisplayName}（{outcome.Error}）"));
                }
            }
        }
        return attempted;
    }

    private void CleanOrphanPackages()
    {
        if (_packages == null) return;
        try
        {
            var referenced = _journal.ReadAll().Select(e => e.PayloadPath);
            _packages.CleanOrphans(referenced, TransferJournal.DefaultTtl);
        }
        catch
        {
            // 孤儿清理失败不影响恢复
        }
    }
}
