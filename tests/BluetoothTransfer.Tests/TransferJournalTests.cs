using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class TransferJournalTests : IDisposable
{
    private readonly string _dir;
    private readonly string _journalPath;
    private readonly PendingPackageStore _packages;

    public TransferJournalTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), $"bt_journal_{Guid.NewGuid():N}");
        Directory.CreateDirectory(_dir);
        _journalPath = Path.Combine(_dir, "send-journal.json");
        _packages = new PendingPackageStore(Path.Combine(_dir, "pending"));
    }

    public void Dispose()
    {
        try { if (Directory.Exists(_dir)) Directory.Delete(_dir, recursive: true); } catch { }
    }

    private string WriteFile(string name, string content)
    {
        var path = Path.Combine(_dir, name);
        File.WriteAllText(path, content);
        return path;
    }

    private static TransferJournalEntry NewEntry(string payloadPath, string displayName = "a.bin",
        string peer = "00:11:22:33:44:55", bool produced = false)
    {
        var info = new FileInfo(payloadPath);
        return new TransferJournalEntry
        {
            Id = Guid.NewGuid().ToString("N"),
            PeerAddr = peer,
            PeerName = "对端",
            DisplayName = displayName,
            PayloadPath = payloadPath,
            SourcePath = payloadPath,
            Kind = TransferJournalKind.File,
            IsProducedPackage = produced,
            Size = info.Length,
            Sha256 = TransferJournal.ComputeSha256(payloadPath)
        };
    }

    private TransferJournal NewJournal(Func<DateTime>? clock = null, IProcessLiveness? liveness = null,
        IPackageCleaner? packages = null)
        => new(new FileTransferJournalStore(_journalPath), clock, packages ?? _packages, liveness);

    // ---------------------------------------------------------------- 写入 / 幂等

    [Fact]
    public void Begin_CreatesEntry_AndPersistsBeforeSend()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();

        var result = journal.Begin(NewEntry(payload));

        Assert.False(result.Busy);
        Assert.False(result.Reused);
        Assert.Equal(TransferJournalState.Sending, result.Entry.State);
        Assert.False(string.IsNullOrEmpty(result.Entry.LeaseUntil));
        Assert.True(result.Entry.OwnerPid > 0);

        // 模拟进程重启后重新读盘：条目仍在
        var reopened = NewJournal().ReadAll();
        Assert.Single(reopened);
        Assert.Equal(payload, reopened[0].PayloadPath);
        Assert.Equal(TransferJournal.ComputeSha256(payload), reopened[0].Sha256);
    }

    [Fact]
    public void Begin_SameDedupeKey_IsIdempotent_NoDuplicateRow()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();

        var first = journal.Begin(NewEntry(payload));
        journal.Release(first.Entry.Id, "连接失败");
        var second = journal.Begin(NewEntry(payload, displayName: "a.bin"));

        Assert.True(second.Reused);
        Assert.Equal(first.Entry.Id, second.Entry.Id);
        Assert.Single(journal.ReadAll());
    }

    [Fact]
    public void Begin_WhileSameEntryLeased_ReturnsBusy_SoNoConcurrentOpen()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();

        journal.Begin(NewEntry(payload));
        var second = journal.Begin(NewEntry(payload));

        Assert.True(second.Busy);
        Assert.Single(journal.ReadAll());
    }

    [Fact]
    public void Begin_DifferentDisplayNameOrPeer_CreatesSeparateEntries()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();

        journal.Begin(NewEntry(payload, "a.bin", "00:11:22:33:44:55"));
        journal.Release(journal.ReadAll()[0].Id, "");

        journal.Begin(NewEntry(payload, "a (1).bin", "00:11:22:33:44:55"));
        journal.Begin(NewEntry(payload, "a.bin", "00:11:22:33:44:66"));

        Assert.Equal(3, journal.ReadAll().Count);
    }

    // ---------------------------------------------------------------- 状态迁移

    [Fact]
    public async Task Complete_RemovesEntry_AndDeletesProducedPackage()
    {
        var package = await _packages.CreateZipFromFileAsync(WriteFile("a.bin", "hello"));
        var journal = NewJournal();
        var begin = journal.Begin(NewEntry(package, "a.bin.zip", produced: true));

        Assert.True(journal.Complete(begin.Entry.Id));

        Assert.Empty(journal.ReadAll());
        Assert.False(File.Exists(package));
    }

    [Fact]
    public void Cancel_RemovesEntry_ButKeepsOriginalSourceFile()
    {
        var source = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        var begin = journal.Begin(NewEntry(source));

        Assert.True(journal.Cancel(begin.Entry.Id));

        Assert.Empty(journal.ReadAll());
        Assert.True(File.Exists(source)); // 原始用户文件绝不删除
    }

    [Fact]
    public void Release_ReturnsToPending_IncrementsAttempts()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        var begin = journal.Begin(NewEntry(payload));

        journal.Release(begin.Entry.Id, "对端不可达");

        var entry = journal.ReadAll().Single();
        Assert.Equal(TransferJournalState.Pending, entry.State);
        Assert.Equal("对端不可达", entry.LastError);
        Assert.Equal(1, entry.Attempts);
        Assert.Equal(0, entry.OwnerPid);
    }

    [Fact]
    public async Task Pause_KeepsEntryAndPackage_ButBlocksAutoResume()
    {
        var package = await _packages.CreateZipFromFileAsync(WriteFile("a.bin", "hello"));
        var journal = NewJournal(liveness: new FakeLiveness(false));
        var begin = journal.Begin(NewEntry(package, "a.bin.zip", produced: true));

        journal.Pause(begin.Entry.Id);

        var plan = journal.PlanRecovery();
        Assert.Empty(plan.Recoverable);
        Assert.Empty(plan.Abandoned);
        Assert.Single(journal.ReadAll());
        Assert.True(File.Exists(package)); // 暂停保留打包产物
        Assert.False(journal.TryClaim(begin.Entry.Id)); // 暂停条目不会被恢复认领
    }

    // ---------------------------------------------------------------- 恢复计划 / 源文件变更

    [Fact]
    public void PlanRecovery_AfterProcessDeath_ImmediatelyRecoverable()
    {
        var payload = WriteFile("a.bin", "hello");
        // journal1 代表“上一个进程”：默认存活判定，条目被它按租约持有
        var before = NewJournal();
        var begin = before.Begin(NewEntry(payload));

        // 重启后：新进程 + 旧 PID 已死 → 无需等 6 分钟租约过期即可恢复
        var after = NewJournal(liveness: new FakeLiveness(false));
        var plan = after.PlanRecovery();

        var entry = Assert.Single(plan.Recoverable);
        Assert.Equal(begin.Entry.Id, entry.Id);
        Assert.Equal(TransferJournal.ComputeSha256(payload), entry.Sha256);
        Assert.Empty(plan.Abandoned);
    }

    [Fact]
    public void PlanRecovery_SkipsEntryHeldByLiveProcess()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        journal.Begin(NewEntry(payload));

        var plan = journal.PlanRecovery();

        Assert.Empty(plan.Recoverable);
        Assert.Empty(plan.Abandoned);
    }

    [Fact]
    public void PlanRecovery_PayloadDeleted_Abandoned()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal(liveness: new FakeLiveness(false));
        journal.Begin(NewEntry(payload));
        File.Delete(payload);

        var plan = journal.PlanRecovery();

        Assert.Empty(plan.Recoverable);
        var abandoned = Assert.Single(plan.Abandoned);
        Assert.Contains("不存在", abandoned.Reason);
    }

    [Fact]
    public void PlanRecovery_SourceContentChangedSameSize_Abandoned()
    {
        var payload = WriteFile("a.bin", "AAAA");
        var journal = NewJournal(liveness: new FakeLiveness(false));
        journal.Begin(NewEntry(payload));
        File.WriteAllText(payload, "BBBB"); // 大小不变、内容变化

        var plan = journal.PlanRecovery();

        Assert.Empty(plan.Recoverable);
        Assert.Contains("SHA-256", Assert.Single(plan.Abandoned).Reason);
    }

    [Fact]
    public void PlanRecovery_SourceSizeChanged_Abandoned()
    {
        var payload = WriteFile("a.bin", "AAAA");
        var journal = NewJournal(liveness: new FakeLiveness(false));
        journal.Begin(NewEntry(payload));
        File.WriteAllText(payload, "AAAA-BBBB");

        var plan = journal.PlanRecovery();

        Assert.Contains("大小已变化", Assert.Single(plan.Abandoned).Reason);
    }

    [Fact]
    public void PlanRecovery_AttemptsExhausted_Abandoned()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal(liveness: new FakeLiveness(false));
        var id = journal.Begin(NewEntry(payload)).Entry.Id;
        for (var i = 0; i < TransferJournal.MaxAttempts; i++) journal.Release(id, "失败");

        var plan = journal.PlanRecovery();

        Assert.Empty(plan.Recoverable);
        Assert.Contains("已尝试", Assert.Single(plan.Abandoned).Reason);
    }

    [Fact]
    public async Task ApplyAbandoned_RemovesEntryAndProducedArtifact()
    {
        var package = await _packages.CreateZipFromFileAsync(WriteFile("a.bin", "hello"));
        var journal = NewJournal(liveness: new FakeLiveness(false));
        journal.Begin(NewEntry(package, "a.bin.zip", produced: true));
        File.Delete(package);

        var plan = journal.PlanRecovery();
        var removed = journal.ApplyAbandoned(plan.Abandoned);

        Assert.Equal(1, removed);
        Assert.Empty(journal.ReadAll());
    }

    [Fact]
    public void TryClaim_TakesOverAfterOwnerDeath_AndRejectsLiveLease()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        var id = journal.Begin(NewEntry(payload)).Entry.Id;

        Assert.False(journal.TryClaim(id)); // 本进程仍活着且持有有效租约

        var after = NewJournal(liveness: new FakeLiveness(false));
        Assert.True(after.TryClaim(id));
        Assert.Equal(TransferJournalState.Sending, after.ReadAll().Single().State);
    }

    // ---------------------------------------------------------------- TTL / 上限

    [Fact]
    public async Task CleanupExpired_RemovesExpiredEntryAndArtifact()
    {
        var now = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc);
        var package = await _packages.CreateZipFromFileAsync(WriteFile("a.bin", "hello"));
        var journal = new TransferJournal(new FileTransferJournalStore(_journalPath),
            () => now, _packages, new FakeLiveness(false));
        journal.Begin(NewEntry(package, "a.bin.zip", produced: true));

        var later = new TransferJournal(new FileTransferJournalStore(_journalPath),
            () => now + TransferJournal.DefaultTtl + TimeSpan.FromMinutes(1), _packages, new FakeLiveness(false));
        var removed = later.CleanupExpired();

        Assert.Equal(1, removed);
        Assert.Empty(later.ReadAll());
        Assert.False(File.Exists(package));
    }

    [Fact]
    public void CleanupExpired_KeepsFreshEntry()
    {
        var now = new DateTime(2026, 1, 1, 0, 0, 0, DateTimeKind.Utc);
        var payload = WriteFile("a.bin", "hello");
        var journal = new TransferJournal(new FileTransferJournalStore(_journalPath),
            () => now, _packages, new FakeLiveness(false));
        journal.Begin(NewEntry(payload));

        Assert.Equal(0, journal.CleanupExpired());
        Assert.Single(journal.ReadAll());
    }

    // ---------------------------------------------------------------- 存储层

    [Fact]
    public void FileStore_ReadAll_ReturnsIndependentCopies()
    {
        var payload = WriteFile("a.bin", "hello");
        var journal = NewJournal(liveness: new FakeLiveness(false));
        journal.Begin(NewEntry(payload));

        var read = journal.ReadAll()[0];
        read.State = "tampered";

        Assert.Equal(TransferJournalState.Sending, journal.ReadAll()[0].State);
    }

    [Fact]
    public void FileStore_CorruptJson_TreatedAsEmpty_AndMovedAside()
    {
        File.WriteAllText(_journalPath, "{ this is not json");

        var store = new FileTransferJournalStore(_journalPath);

        Assert.Empty(store.ReadAll());
        Assert.True(File.Exists(_journalPath + ".corrupt"));
    }

    [Fact]
    public void FileStore_MissingFile_ReturnsEmpty()
    {
        var store = new FileTransferJournalStore(Path.Combine(_dir, "none.json"));
        Assert.Empty(store.ReadAll());
    }

    [Fact]
    public void FileStore_AtomicWrite_LeavesNoTempFile()
    {
        var payload = WriteFile("a.bin", "hello");
        NewJournal().Begin(NewEntry(payload));

        Assert.True(File.Exists(_journalPath));
        Assert.False(File.Exists(_journalPath + ".tmp"));
    }

    [Fact]
    public void InMemoryStore_And_FileStore_BehaveTheSame()
    {
        var payload = WriteFile("a.bin", "hello");
        var fileJournal = NewJournal(liveness: new FakeLiveness(false));
        var memoryJournal = new TransferJournal(new InMemoryJournalStore(), liveness: new FakeLiveness(false));

        var a = fileJournal.Begin(NewEntry(payload));
        var b = memoryJournal.Begin(NewEntry(payload));

        Assert.Equal(a.Entry.DisplayName, b.Entry.DisplayName);
        Assert.Equal(a.Entry.Sha256, b.Entry.Sha256);
        Assert.Equal(a.Entry.Size, b.Entry.Size);
        Assert.Single(fileJournal.ReadAll());
        Assert.Single(memoryJournal.ReadAll());
    }
}
