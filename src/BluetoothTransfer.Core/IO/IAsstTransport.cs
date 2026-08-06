namespace BluetoothTransfer.Core.IO;

/// <summary>
/// 私有助手协议传输抽象：真实实现走 RFCOMM StreamSocket，
/// 测试/自检使用内存实现（MemoryAsstTransport）。
/// </summary>
public interface IAsstTransport : IAsyncDisposable
{
    /// <summary>写入全部字节；返回时数据已提交到传输层。</summary>
    Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default);

    /// <summary>读取最多 buffer.Length 字节，返回实际读取数；0 表示对端已关闭。</summary>
    Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default);
}
