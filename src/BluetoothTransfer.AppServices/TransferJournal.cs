using System.Globalization;
using System.Security.Cryptography;
using System.Text.Json.Serialization;

namespace BluetoothTransfer.Services;

/// <summary>
/// 发送日志条目的状态机取值：
/// pending（待发送，允许启动恢复自动重发）/ sending（已被某次运行按租约占用，租约过期即视为进程崩溃，可恢复）/
/// paused（用户显式暂停，保留条目与打包产物，但启动恢复不自动重发，等用户点“继续”）。
/// </summary>
public static class TransferJournalState
{
    public const string Pending = "pending";
    public const string Sending = "sending";
    public const string Paused = "paused";
}

/// <summary>发送日志条目记录的来源类型（仅用于诊断/展示，不参与协议）。</summary>
public static class TransferJournalKind
{
    public const string File = "file";
    public const string FileZip = "file-zip";
    public const string Folder = "folder";
    public const string Text = "text";
}

/// <summary>
/// 持有者进程存活判定：用于区分“进程被杀的残留条目”（可立即恢复）与“另一进程正在发送”（必须跳过）。
/// 抽成接口以便单测注入（Linux 上模拟“进程已死/重启”）。
/// </summary>
public interface IProcessLiveness
{
    bool IsAlive(int pid, DateTime startUtc);
}

/// <summary>基于 System.Diagnostics.Process 的存活判定：PID 存在且启动时间匹配才算“同一进程”，避免 PID 复用误判。</summary>
public sealed class SystemProcessLiveness : IProcessLiveness
{
    public bool IsAlive(int pid, DateTime startUtc)
    {
        if (pid <= 0) return false;
        try
        {
            using var process = System.Diagnostics.Process.GetProcessById(pid);
            if (startUtc == default) return true;
            return Math.Abs((process.StartTime.ToUniversalTime() - startUtc).TotalSeconds) < 2;
        }
        catch
        {
            return false;
        }
    }
}

/// <summary>
/// 发送端“未完成传输”的持久化记录（纯 POCO，可直接 JSON 序列化）。
/// 一条记录对应一次跨进程可续传的助手通道传输：重启后按 <see cref="PayloadPath"/> 重新发起，
/// 接收端按同样的 DisplayName + Sha256(=transferId) + Size 命中半成品并回 OFFER(ResumeOffset) 续传。
/// </summary>
public sealed class TransferJournalEntry
{
    /// <summary>条目 ID（GUID），journal 内部主键。</summary>
    public string Id { get; set; } = "";
    /// <summary>对端蓝牙地址。</summary>
    public string PeerAddr { get; set; } = "";
    /// <summary>对端显示名（尽量记录，找不到设备时可能为空）。</summary>
    public string PeerName { get; set; } = "";
    /// <summary>对端落盘文件名：续传命中的关键之一，恢复时必须与首次完全一致。</summary>
    public string DisplayName { get; set; } = "";
    /// <summary>待发送字节数。</summary>
    public long Size { get; set; }
    /// <summary>内容 SHA-256（十六进制大写），同时作为协议 transferId。</summary>
    public string Sha256 { get; set; } = "";
    /// <summary>实际发送的本地文件：原始文件，或保留在待续传目录的打包产物。</summary>
    public string PayloadPath { get; set; } = "";
    /// <summary>用户原始来源（文件/文件夹路径；文本为空），用于失败记录与诊断。</summary>
    public string SourcePath { get; set; } = "";
    /// <summary>来源类型，见 <see cref="TransferJournalKind"/>。</summary>
    public string Kind { get; set; } = TransferJournalKind.File;
    /// <summary>PayloadPath 是否为本次生成的打包产物（完成/取消/放弃后需要删除）。</summary>
    public bool IsProducedPackage { get; set; }
    /// <summary>状态，见 <see cref="TransferJournalState"/>。</summary>
    public string State { get; set; } = TransferJournalState.Pending;
    /// <summary>租约到期时间（UTC ISO-8601）；仅 sending 状态有效。</summary>
    public string LeaseUntil { get; set; } = "";
    /// <summary>持有者进程 PID；进程死亡后条目可被立即恢复，不必等租约到期。</summary>
    public int OwnerPid { get; set; }
    /// <summary>持有者进程启动时间（UTC ISO-8601），与 PID 合起来避免 PID 复用误判。</summary>
    public string OwnerStartUtc { get; set; } = "";
    /// <summary>最近一次已确认偏移（诊断用；真正的续传偏移由接收端 OFFER 返回）。</summary>
    public long ResumeOffset { get; set; }
    /// <summary>已被发送/恢复尝试的次数，达到上限后放弃。</summary>
    public int Attempts { get; set; }
    /// <summary>最近一次失败原因。</summary>
    public string LastError { get; set; } = "";
    public string CreatedAt { get; set; } = "";
    public string UpdatedAt { get; set; } = "";

    /// <summary>
    /// 幂等键：对端 + 内容 SHA-256 + 落盘文件名。
    /// 必须与接收端半成品命中条件一致（AsstFileSink 以文件名定位 .btpart，以 TransferId+FileSize 校验内容）。
    /// </summary>
    [JsonIgnore]
    public string DedupeKey => PeerAddr + "|" + Sha256 + "|" + DisplayName;

    public TransferJournalEntry Clone() => (TransferJournalEntry)MemberwiseClone();
}

/// <summary>
/// journal 持久化抽象：读全量 + 在跨进程独占锁内做一次“读-改-写”。
/// Mutate 是唯一的写入口，保证状态迁移（认领/续租/释放/删除）之间的竞态不会丢更新。
/// </summary>
public interface ITransferJournalStore
{
    IReadOnlyList<TransferJournalEntry> ReadAll();

    /// <summary>在独占锁内读取全部条目、执行 <paramref name="mutate"/>，随后原子落盘。</summary>
    T Mutate<T>(Func<List<TransferJournalEntry>, T> mutate);
}

/// <summary>打包产物清理抽象：journal 删除条目时顺带清理其生成的打包产物，避免遗留孤儿文件。</summary>
public interface IPackageCleaner
{
    bool Delete(string path);
}

/// <summary><see cref="TransferJournal.Begin"/> 的结果。</summary>
public sealed record JournalBeginResult(
    TransferJournalEntry Entry,
    bool Reused,
    bool Busy,
    string SupersededPayloadPath = "",
    bool SupersededWasProduced = false);

/// <summary>被放弃的条目及原因（源文件变更/删除、超过保留期、重试次数耗尽）。</summary>
public sealed record JournalAbandoned(TransferJournalEntry Entry, string Reason);

/// <summary>一次恢复计划：可恢复条目（已通过载荷校验）与应放弃条目。</summary>
public sealed record JournalRecoveryPlan(
    IReadOnlyList<TransferJournalEntry> Recoverable,
    IReadOnlyList<JournalAbandoned> Abandoned);

/// <summary>
/// 发送日志（durable send journal）：持久化“未完成的助手通道传输”，使进程被杀/断电/重启后仍能
/// 从接收端已确认的偏移继续。纯逻辑类型——不依赖 Windows/WinRT/WPF，可在任意平台单测。
/// </summary>
/// <remarks>
/// 并发正确性：所有状态迁移都经 <see cref="ITransferJournalStore.Mutate"/> 在跨进程独占锁内完成；
/// 任何一次实际发送都先通过 <see cref="Begin"/> 或 <see cref="TryClaim"/> 取得租约（sending + OwnerPid/OwnerStartUtc + LeaseUntil），
/// 持有者进程仍存活的条目一律拒绝重复发起，因此同一 (对端, 文件名, 内容 SHA) 不会被并发双开。
/// 进程被杀后 PID 已不存在，重启即可立即接管，无需等待租约到期；租约仅作为缺少持有者信息时的兜底。
/// </remarks>
public sealed class TransferJournal
{
    /// <summary>条目与打包产物的保留期，与接收端 AsstFileSink.CleanStale 的 7 天口径对齐。</summary>
    public static readonly TimeSpan DefaultTtl = TimeSpan.FromDays(7);

    /// <summary>发送租约基准时长：进程崩溃但 PID 复用等极端情况下，最长 6 分钟即可被启动恢复接管。</summary>
    public static readonly TimeSpan LeaseDuration = TimeSpan.FromMinutes(6);

    /// <summary>租约续期间隔：长传输期间按此频率续租，避免租约在发送途中过期导致被并发双开。</summary>
    public static readonly TimeSpan RenewInterval = TimeSpan.FromMinutes(1);

    /// <summary>单条目最大尝试次数，超过则放弃（避免僵尸条目无限重试）。</summary>
    public const int MaxAttempts = 6;

    /// <summary>条目数上限，超出按创建时间淘汰最旧（有效租约/存活持有者的除外）。</summary>
    public const int MaxEntries = 200;

    private static readonly Lazy<(int Pid, DateTime StartUtc)> CurrentProcess = new(() =>
    {
        try
        {
            using var process = System.Diagnostics.Process.GetCurrentProcess();
            return (process.Id, process.StartTime.ToUniversalTime());
        }
        catch
        {
            return (0, default);
        }
    });

    private readonly ITransferJournalStore _store;
    private readonly Func<DateTime> _utcNow;
    private readonly IPackageCleaner? _packages;
    private readonly IProcessLiveness _liveness;

    public TransferJournal(
        ITransferJournalStore store,
        Func<DateTime>? utcNow = null,
        IPackageCleaner? packages = null,
        IProcessLiveness? liveness = null)
    {
        _store = store ?? throw new ArgumentNullException(nameof(store));
        _utcNow = utcNow ?? (() => DateTime.UtcNow);
        _packages = packages;
        _liveness = liveness ?? new SystemProcessLiveness();
    }

    public IReadOnlyList<TransferJournalEntry> ReadAll() => _store.ReadAll();

    /// <summary>
    /// 开始（或接管）一次发送：按 <see cref="TransferJournalEntry.DedupeKey"/> 幂等去重，
    /// 无既有条目则新增，有则复用同一条目（不产生重复行），并取得发送租约。
    /// 若既有条目仍被有效租约/存活进程占用，返回 <c>Busy=true</c>，调用方必须放弃本次重复发起。
    /// </summary>
    public JournalBeginResult Begin(TransferJournalEntry entry)
    {
        ArgumentNullException.ThrowIfNull(entry);
        var now = _utcNow();
        List<TransferJournalEntry> removed = new();
        var result = _store.Mutate(list =>
        {
            Sweep(list, now, removed);
            var existing = list.FirstOrDefault(e => e.DedupeKey == entry.DedupeKey);
            if (existing != null)
            {
                if (IsBusy(existing, now))
                    return new JournalBeginResult(existing.Clone(), Reused: true, Busy: true);

                var oldPayload = existing.PayloadPath;
                var oldProduced = existing.IsProducedPackage;
                existing.PeerName = entry.PeerName;
                existing.DisplayName = entry.DisplayName;
                existing.Size = entry.Size;
                existing.Sha256 = entry.Sha256;
                existing.PayloadPath = entry.PayloadPath;
                existing.SourcePath = entry.SourcePath;
                existing.Kind = entry.Kind;
                existing.IsProducedPackage = entry.IsProducedPackage;
                existing.ResumeOffset = 0;
                existing.LastError = "";
                Claim(existing, now);
                var superseded = oldProduced
                    && !string.IsNullOrEmpty(oldPayload)
                    && !string.Equals(oldPayload, existing.PayloadPath, StringComparison.OrdinalIgnoreCase);
                return new JournalBeginResult(existing.Clone(), Reused: true, Busy: false, oldPayload, superseded);
            }

            if (string.IsNullOrEmpty(entry.Id)) entry.Id = Guid.NewGuid().ToString("N");
            if (string.IsNullOrEmpty(entry.CreatedAt)) entry.CreatedAt = Iso(now);
            entry.ResumeOffset = 0;
            entry.LastError = "";
            Claim(entry, now);
            list.Add(entry);
            return new JournalBeginResult(entry.Clone(), Reused: false, Busy: false);
        });

        CleanupArtifacts(removed);
        if (result.SupersededWasProduced) DeleteArtifact(result.SupersededPayloadPath);
        return result;
    }

    /// <summary>
    /// 认领一条既有条目用于恢复：pending 或“持有者进程已死”的 sending 可认领；
    /// paused 与仍被存活进程/有效租约占用的条目拒绝。返回 false 表示不该由本次运行发送。
    /// </summary>
    public bool TryClaim(string id)
    {
        var now = _utcNow();
        List<TransferJournalEntry> removed = new();
        var ok = _store.Mutate(list =>
        {
            Sweep(list, now, removed);
            var entry = list.FirstOrDefault(e => e.Id == id);
            if (entry == null) return false;
            if (entry.State == TransferJournalState.Paused) return false;
            if (IsBusy(entry, now)) return false;
            Claim(entry, now);
            return true;
        });
        CleanupArtifacts(removed);
        return ok;
    }

    /// <summary>续租：长传输期间刷新租约，防止被启动恢复判定为崩溃残留（PID 存活判定是第二道保险）。</summary>
    public void Renew(string id)
    {
        var now = _utcNow();
        _store.Mutate(list =>
        {
            var entry = list.FirstOrDefault(e => e.Id == id);
            if (entry != null && entry.State == TransferJournalState.Sending)
            {
                entry.LeaseUntil = Iso(now + LeaseDuration);
                entry.UpdatedAt = Iso(now);
            }
            return 0;
        });
    }

    /// <summary>发送失败但可下次恢复：回到 pending 并累加尝试次数（保留条目与打包产物）。</summary>
    public bool Release(string id, string error, bool countAttempt = true)
    {
        var now = _utcNow();
        return _store.Mutate(list =>
        {
            var entry = list.FirstOrDefault(e => e.Id == id);
            if (entry == null) return false;
            entry.State = TransferJournalState.Pending;
            entry.LeaseUntil = "";
            entry.OwnerPid = 0;
            entry.OwnerStartUtc = "";
            entry.LastError = error ?? "";
            if (countAttempt) entry.Attempts++;
            entry.UpdatedAt = Iso(now);
            return true;
        });
    }

    /// <summary>用户暂停：保留条目与打包产物，但不参与启动恢复自动重发。</summary>
    public bool Pause(string id)
    {
        var now = _utcNow();
        return _store.Mutate(list =>
        {
            var entry = list.FirstOrDefault(e => e.Id == id);
            if (entry == null) return false;
            entry.State = TransferJournalState.Paused;
            entry.LeaseUntil = "";
            entry.OwnerPid = 0;
            entry.OwnerStartUtc = "";
            entry.UpdatedAt = Iso(now);
            return true;
        });
    }

    /// <summary>成功完成：删除条目并清理其打包产物。</summary>
    public bool Complete(string id) => Remove(id);

    /// <summary>用户取消：删除条目并清理其打包产物。</summary>
    public bool Cancel(string id) => Remove(id);

    /// <summary>删除条目（含打包产物清理）。</summary>
    public bool Remove(string id)
    {
        TransferJournalEntry? removed = null;
        var ok = _store.Mutate(list =>
        {
            var entry = list.FirstOrDefault(e => e.Id == id);
            if (entry == null) return false;
            list.Remove(entry);
            removed = entry;
            return true;
        });
        if (removed != null) CleanupArtifacts(new[] { removed });
        return ok;
    }

    /// <summary>更新已确认偏移（诊断用，不参与续传协商）。</summary>
    public void MarkProgress(string id, long offset)
    {
        var now = _utcNow();
        _store.Mutate(list =>
        {
            var entry = list.FirstOrDefault(e => e.Id == id);
            if (entry != null)
            {
                entry.ResumeOffset = offset;
                entry.UpdatedAt = Iso(now);
            }
            return 0;
        });
    }

    /// <summary>
    /// 制定恢复计划：跳过 paused 与仍被存活进程/有效租约占用的 sending；
    /// 对源文件已删除/变更（大小或 SHA 不一致）、超过保留期、尝试次数耗尽的条目判为放弃。
    /// </summary>
    public JournalRecoveryPlan PlanRecovery()
    {
        var now = _utcNow();
        var recoverable = new List<TransferJournalEntry>();
        var abandoned = new List<JournalAbandoned>();
        foreach (var entry in _store.ReadAll())
        {
            if (entry.State == TransferJournalState.Paused) continue;
            if (IsBusy(entry, now)) continue;
            if (IsExpired(entry, now))
            {
                abandoned.Add(new JournalAbandoned(entry, "已超过 7 天保留期，放弃断点续传"));
                continue;
            }
            if (entry.Attempts >= MaxAttempts)
            {
                abandoned.Add(new JournalAbandoned(entry, $"已尝试 {entry.Attempts} 次仍未完成，放弃断点续传"));
                continue;
            }
            if (!ValidatePayload(entry, out var reason))
            {
                abandoned.Add(new JournalAbandoned(entry, reason));
                continue;
            }
            recoverable.Add(entry.Clone());
        }
        return new JournalRecoveryPlan(recoverable, abandoned);
    }

    /// <summary>删除已放弃的条目并清理其打包产物，返回删除条数。</summary>
    public int ApplyAbandoned(IEnumerable<JournalAbandoned> abandoned)
    {
        ArgumentNullException.ThrowIfNull(abandoned);
        var ids = abandoned.Select(a => a.Entry.Id)
            .Where(id => !string.IsNullOrEmpty(id))
            .Distinct(StringComparer.Ordinal)
            .ToList();
        if (ids.Count == 0) return 0;
        List<TransferJournalEntry> removed = new();
        var count = _store.Mutate(list =>
        {
            var n = 0;
            foreach (var id in ids)
            {
                var entry = list.FirstOrDefault(e => e.Id == id);
                if (entry == null) continue;
                list.Remove(entry);
                removed.Add(entry);
                n++;
            }
            return n;
        });
        CleanupArtifacts(removed);
        return count;
    }

    /// <summary>清理超过保留期与超出条目上限的条目（跳过存活持有者），返回清理条数。</summary>
    public int CleanupExpired()
    {
        var now = _utcNow();
        List<TransferJournalEntry> removed = new();
        var count = _store.Mutate(list =>
        {
            Sweep(list, now, removed);
            return removed.Count;
        });
        CleanupArtifacts(removed);
        return count;
    }

    /// <summary>
    /// 校验载荷可续传：文件存在、大小与 SHA-256 均与记录一致。
    /// 恢复时必须原样发送同一份字节，否则接收端 OFFER 不会命中半成品。
    /// </summary>
    public static bool ValidatePayload(TransferJournalEntry entry, out string reason)
    {
        reason = "";
        if (entry == null) { reason = "条目为空"; return false; }
        if (string.IsNullOrWhiteSpace(entry.PayloadPath)) { reason = "缺少待发送文件路径"; return false; }
        if (!File.Exists(entry.PayloadPath)) { reason = $"待发送文件已不存在：{entry.PayloadPath}"; return false; }
        long length;
        try { length = new FileInfo(entry.PayloadPath).Length; }
        catch (Exception ex) { reason = $"待发送文件不可读：{ex.Message}"; return false; }
        if (length != entry.Size)
        {
            reason = $"源文件大小已变化（{entry.Size} -> {length}），无法续传";
            return false;
        }
        string sha;
        try { sha = ComputeSha256(entry.PayloadPath); }
        catch (Exception ex) { reason = $"计算校验和失败：{ex.Message}"; return false; }
        if (!string.Equals(sha, entry.Sha256, StringComparison.OrdinalIgnoreCase))
        {
            reason = "源文件内容已变化（SHA-256 不一致），无法续传";
            return false;
        }
        return true;
    }

    /// <summary>文件内容 SHA-256（十六进制大写，与 FileChecksum/接收端比较口径一致）。</summary>
    public static string ComputeSha256(string path)
    {
        using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        return Convert.ToHexString(SHA256.HashData(fs));
    }

    private void Claim(TransferJournalEntry entry, DateTime now)
    {
        var (pid, startUtc) = CurrentProcess.Value;
        entry.State = TransferJournalState.Sending;
        entry.LeaseUntil = Iso(now + LeaseDuration);
        entry.OwnerPid = pid;
        entry.OwnerStartUtc = startUtc == default ? "" : Iso(startUtc);
        entry.UpdatedAt = Iso(now);
    }

    private static bool IsLeaseValid(TransferJournalEntry entry, DateTime now)
        => entry.State == TransferJournalState.Sending
           && TryParseIso(entry.LeaseUntil, out var until)
           && until > now;

    /// <summary>
    /// 条目是否仍被“存活的持有者进程”占用（占用中一律不允许重复发起）。
    /// 判定顺序很关键：有持有者信息时以进程存活为准 —— 进程被杀后即使租约尚未到期也立即可恢复（这是本 bug 的核心场景）；
    /// 仅当条目缺少持有者信息（旧版本写入）时才回退到租约到期判定。
    /// </summary>
    private bool IsBusy(TransferJournalEntry entry, DateTime now)
    {
        if (entry.State != TransferJournalState.Sending) return false;
        if (entry.OwnerPid > 0 && TryParseIso(entry.OwnerStartUtc, out var startUtc))
            return _liveness.IsAlive(entry.OwnerPid, startUtc);
        return IsLeaseValid(entry, now);
    }

    private static bool IsExpired(TransferJournalEntry entry, DateTime now)
        => TryParseIso(entry.UpdatedAt, out var updated) && updated < now - DefaultTtl;

    /// <summary>TTL 与条目上限清理（在 Mutate 内执行，避免与并发状态迁移互相覆盖）。</summary>
    private void Sweep(List<TransferJournalEntry> list, DateTime now, List<TransferJournalEntry> removed)
    {
        foreach (var entry in list.ToList())
        {
            if (IsBusy(entry, now)) continue;
            if (IsExpired(entry, now))
            {
                list.Remove(entry);
                removed.Add(entry);
            }
        }
        if (list.Count <= MaxEntries) return;
        var overflow = list
            .Where(e => !IsBusy(e, now))
            .OrderBy(e => e.CreatedAt, StringComparer.Ordinal)
            .ThenBy(e => e.Id, StringComparer.Ordinal)
            .Take(list.Count - MaxEntries)
            .ToList();
        foreach (var entry in overflow)
        {
            list.Remove(entry);
            removed.Add(entry);
        }
    }

    private void CleanupArtifacts(IEnumerable<TransferJournalEntry>? entries)
    {
        if (entries == null) return;
        foreach (var entry in entries)
        {
            if (entry.IsProducedPackage) DeleteArtifact(entry.PayloadPath);
        }
    }

    private void DeleteArtifact(string? path)
    {
        if (_packages == null || string.IsNullOrWhiteSpace(path)) return;
        try { _packages.Delete(path!); }
        catch { /* 产物清理失败不影响 journal 一致性 */ }
    }

    private static bool TryParseIso(string value, out DateTime utc)
    {
        if (DateTime.TryParse(value, CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind, out var parsed))
        {
            utc = parsed.ToUniversalTime();
            return true;
        }
        utc = default;
        return false;
    }

    private static string Iso(DateTime time) => time.ToUniversalTime().ToString("o", CultureInfo.InvariantCulture);
}
