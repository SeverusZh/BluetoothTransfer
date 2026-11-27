using BluetoothTransfer.Services;

namespace BluetoothTransfer.Tests;

/// <summary>内存 journal 存储：模拟一次“读-改-写”事务，用于纯逻辑测试。</summary>
internal sealed class InMemoryJournalStore : ITransferJournalStore
{
    private readonly List<TransferJournalEntry> _entries = new();

    public IReadOnlyList<TransferJournalEntry> ReadAll() => _entries.Select(e => e.Clone()).ToList();

    public T Mutate<T>(Func<List<TransferJournalEntry>, T> mutate)
    {
        var list = _entries.Select(e => e.Clone()).ToList();
        var result = mutate(list);
        _entries.Clear();
        _entries.AddRange(list);
        return result;
    }
}

/// <summary>可注入的持有者进程存活判定：用于模拟“进程已被杀 / 已重启”。</summary>
internal sealed class FakeLiveness : IProcessLiveness
{
    private readonly bool _alive;

    public FakeLiveness(bool alive) => _alive = alive;

    public bool IsAlive(int pid, DateTime startUtc) => _alive;
}

/// <summary>记录收到的 journal 条目并返回预设结果的 fake 发送器。</summary>
internal sealed class RecordingSender : IJournalEntrySender
{
    private readonly Func<TransferJournalEntry, JournalSendOutcome> _handler;

    public RecordingSender(Func<TransferJournalEntry, JournalSendOutcome>? handler = null)
        => _handler = handler ?? (e => JournalSendOutcome.Success(e.Size, 0));

    public List<TransferJournalEntry> Sent { get; } = new();

    public Task<JournalSendOutcome> SendJournalEntryAsync(TransferJournalEntry entry, CancellationToken ct = default)
    {
        Sent.Add(entry.Clone());
        return Task.FromResult(_handler(entry));
    }
}
