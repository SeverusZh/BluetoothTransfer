using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class SendResumeServiceTests : IDisposable
{
    private readonly string _dir;
    private readonly string _journalPath;
    private readonly PendingPackageStore _packages;

    public SendResumeServiceTests()
    {
        _dir = Path.Combine(Path.GetTempPath(), $"bt_resume_{Guid.NewGuid():N}");
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

    /// <summary>模拟“上一个进程已死”的 journal（租约仍在有效期内，但持有者 PID 已不存在）。</summary>
    private TransferJournal NewJournal(bool ownerAlive = false)
        => new(new FileTransferJournalStore(_journalPath), packages: _packages, liveness: new FakeLiveness(ownerAlive));

    private static TransferJournalEntry EntryFor(string payloadPath, string displayName, bool produced)
        => new()
        {
            Id = Guid.NewGuid().ToString("N"),
            PeerAddr = "00:11:22:33:44:55",
            PeerName = "对端",
            DisplayName = displayName,
            PayloadPath = payloadPath,
            SourcePath = payloadPath,
            Kind = produced ? TransferJournalKind.FileZip : TransferJournalKind.File,
            IsProducedPackage = produced,
            Size = new FileInfo(payloadPath).Length,
            Sha256 = TransferJournal.ComputeSha256(payloadPath)
        };

    [Fact]
    public async Task Resume_Success_CompletesJournal_DeletesPackage_AndReusesIdentity()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        var sha = TransferJournal.ComputeSha256(package);
        var journal = NewJournal();
        journal.Begin(EntryFor(package, "a.bin.zip", produced: true));

        var sender = new RecordingSender(e => JournalSendOutcome.Success(e.Size, 4096));
        var service = new SendResumeService(journal, sender, packages: _packages);

        var attempted = await service.ResumeAllAsync();

        Assert.Equal(1, attempted);
        var sent = Assert.Single(sender.Sent);
        Assert.Equal("a.bin.zip", sent.DisplayName);   // 文件名必须与首次一致，否则接收端 .btpart 不命中
        Assert.Equal(sha, sent.Sha256);                // transferId 必须与首次一致
        Assert.Equal(package, sent.PayloadPath);
        Assert.Empty(journal.ReadAll());
        Assert.False(File.Exists(package));            // 成功后清理打包产物
    }

    [Fact]
    public async Task Resume_AfterRestart_FromAnotherProcessJournal()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        // 上一个进程：默认存活判定，写入条目后“被杀”
        var before = new TransferJournal(new FileTransferJournalStore(_journalPath), packages: _packages);
        before.Begin(EntryFor(package, "a.bin.zip", produced: true));

        // 重启后进程：旧 PID 已死，立即可恢复（无需等 6 分钟租约）
        var after = NewJournal();
        var sender = new RecordingSender();
        var service = new SendResumeService(after, sender, packages: _packages);

        var attempted = await service.ResumeAllAsync();

        Assert.Equal(1, attempted);
        Assert.Single(sender.Sent);
        Assert.Empty(after.ReadAll());
    }

    [Fact]
    public async Task Resume_Failure_KeepsEntryPending_AndKeepsPackage()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        var journal = NewJournal();
        journal.Begin(EntryFor(package, "a.bin.zip", produced: true));

        var sender = new RecordingSender(_ => JournalSendOutcome.Failure("对端不可达"));
        var service = new SendResumeService(journal, sender, packages: _packages);

        await service.ResumeAllAsync();

        var entry = Assert.Single(journal.ReadAll());
        Assert.Equal(TransferJournalState.Pending, entry.State);
        Assert.Equal(1, entry.Attempts);
        Assert.Equal("对端不可达", entry.LastError);
        Assert.True(File.Exists(package)); // 未完成前保留打包产物，保证下次 SHA 仍可复现
    }

    [Fact]
    public async Task Resume_Repeated_IsIdempotent_NoDuplicateRows()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        var journal = NewJournal();
        journal.Begin(EntryFor(package, "a.bin.zip", produced: true));

        var sender = new RecordingSender(_ => JournalSendOutcome.Failure("超时"));
        var service = new SendResumeService(journal, sender, packages: _packages);

        await service.ResumeAllAsync();
        await service.ResumeAllAsync();

        Assert.Equal(2, sender.Sent.Count);
        Assert.Equal(2, Assert.Single(journal.ReadAll()).Attempts); // 仍是同一条目，未重复创建
    }

    [Fact]
    public async Task Resume_PausedEntry_IsNotSent()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        var journal = NewJournal();
        var id = journal.Begin(EntryFor(package, "a.bin.zip", produced: true)).Entry.Id;
        journal.Pause(id);

        var sender = new RecordingSender();
        var service = new SendResumeService(journal, sender, packages: _packages);

        var attempted = await service.ResumeAllAsync();

        Assert.Equal(0, attempted);
        Assert.Empty(sender.Sent);
        Assert.Equal(TransferJournalState.Paused, Assert.Single(journal.ReadAll()).State);
        Assert.True(File.Exists(package));
    }

    [Fact]
    public async Task Resume_DeletedPayload_Abandoned_NotSent()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        var journal = NewJournal();
        journal.Begin(EntryFor(package, "a.bin.zip", produced: true));
        File.Delete(package); // 源（或打包产物）已删除

        var sender = new RecordingSender();
        var abandoned = new List<string>();
        var service = new SendResumeService(journal, sender, packages: _packages,
            onAbandoned: (entry, reason) => abandoned.Add(reason));

        var attempted = await service.ResumeAllAsync();

        Assert.Equal(0, attempted);
        Assert.Empty(sender.Sent);
        Assert.Contains("不存在", Assert.Single(abandoned));
        Assert.Empty(journal.ReadAll());
    }

    [Fact]
    public async Task Resume_ChangedSource_Abandoned_NotSent()
    {
        var source = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        journal.Begin(EntryFor(source, "a.bin", produced: false));
        File.WriteAllText(source, "hello-world"); // 源文件已变更

        var sender = new RecordingSender();
        var abandoned = new List<string>();
        var service = new SendResumeService(journal, sender, packages: _packages,
            onAbandoned: (entry, reason) => abandoned.Add(reason));

        var attempted = await service.ResumeAllAsync();

        Assert.Equal(0, attempted);
        Assert.Empty(sender.Sent);
        Assert.Single(abandoned);
        Assert.True(File.Exists(source)); // 不删除用户原始文件
    }

    [Fact]
    public async Task Resume_Cancelled_KeepsEntryForNextStart()
    {
        var source = WriteFile("a.bin", "hello");
        var package = await _packages.CreateZipFromFileAsync(source);
        var journal = NewJournal();
        journal.Begin(EntryFor(package, "a.bin.zip", produced: true));

        var sender = new RecordingSender(_ => JournalSendOutcome.CancelledOutcome());
        var service = new SendResumeService(journal, sender, packages: _packages);

        await service.ResumeAllAsync();

        var entry = Assert.Single(journal.ReadAll());
        Assert.Equal(TransferJournalState.Pending, entry.State);
        Assert.Equal(0, entry.Attempts); // 应用退出导致的取消不计入失败尝试
    }

    [Fact]
    public async Task Resume_SenderThrows_TreatedAsRetryableFailure()
    {
        var source = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        journal.Begin(EntryFor(source, "a.bin", produced: false));

        var sender = new RecordingSender(_ => throw new InvalidOperationException("boom"));
        var service = new SendResumeService(journal, sender, packages: _packages);

        await service.ResumeAllAsync();

        var entry = Assert.Single(journal.ReadAll());
        Assert.Equal(TransferJournalState.Pending, entry.State);
        Assert.Contains("boom", entry.LastError);
    }

    [Fact]
    public async Task Resume_SecondRunAfterSuccess_DoesNothing()
    {
        var source = WriteFile("a.bin", "hello");
        var journal = NewJournal();
        journal.Begin(EntryFor(source, "a.bin", produced: false));

        var sender = new RecordingSender();
        var service = new SendResumeService(journal, sender, packages: _packages);

        await service.ResumeAllAsync();
        var second = await service.ResumeAllAsync();

        Assert.Equal(0, second);
        Assert.Single(sender.Sent);
        Assert.Empty(journal.ReadAll());
    }
}
