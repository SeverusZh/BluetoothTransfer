using BluetoothTransfer.Core.Protocol;

namespace BluetoothTransfer.Core.Server;

/// <summary>接收端数据落盘抽象：负责半成品、元数据、完成改名与校验。</summary>
public interface IAsstSink
{
    /// <summary>打开接收（含续传协商）：返回已落盘的偏移量（0 表示全新开始）。</summary>
    Task<long> OpenAsync(AsstHello hello, CancellationToken ct = default);

    /// <summary>按偏移追加写入并落盘；offset 必须等于当前已写长度。</summary>
    Task WriteAsync(AsstHello hello, long offset, ReadOnlyMemory<byte> data, CancellationToken ct = default);

    /// <summary>完成：校验 SHA-256 并改名；不匹配时清空半成品并返回 Ok=false。</summary>
    Task<AsstDone> CompleteAsync(AsstHello hello, CancellationToken ct = default);

    /// <summary>中止：deletePartial=true 时删除半成品与元数据（哈希不匹配/协议错误）；false 时保留（可续传）。</summary>
    Task AbortAsync(AsstHello hello, bool deletePartial, CancellationToken ct = default);
}
