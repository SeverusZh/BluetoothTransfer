using System.Collections.Concurrent;
using System.IO;
using System.Text;

namespace BluetoothTransfer.Services;

/// <summary>
/// BLE 收发两端共用的应用层分片重组器。
/// 按 <see cref="Frame.TaskId"/> 缓冲 DATA 分片（依据 Offset 有序写入），遇到 FinalChunk/END 收尾：
/// 存在 type="file" 的 META 时按二进制文件发布 <see cref="FileDataReceivedEvent"/>，否则按 UTF-8 文本发布 <see cref="TextReceivedEvent"/>。
/// 加密分片（FrameFlags.Encrypted）在写入缓冲前解密。
/// </summary>
public class FrameReassembler
{
    private sealed record FileMeta(string Name, long Size, string Checksum, bool Compressed);

    private readonly EventBus _eventBus;
    private readonly CryptoService _crypto;
    private readonly ConcurrentDictionary<uint, MemoryStream> _buffers = new();
    private readonly ConcurrentDictionary<uint, FileMeta> _fileMeta = new();

    public FrameReassembler(EventBus eventBus, CryptoService crypto)
    {
        _eventBus = eventBus;
        _crypto = crypto;
    }

    public void HandleFrame(Frame frame, string channel, string peerAddr, string peerName)
    {
        switch (frame.MsgType)
        {
            case MsgType.META:
                HandleMeta(frame);
                break;
            case MsgType.DATA:
                HandleData(frame, channel, peerAddr, peerName);
                break;
            case MsgType.END:
                Finalize(frame.TaskId, channel, peerAddr, peerName);
                break;
        }
    }

    private void HandleMeta(Frame frame)
    {
        var meta = MetaPayload.Decode(frame.Payload);
        if (meta == null) return;

        if (meta.Value.type == "file")
        {
            var compressed = (frame.Flags & FrameFlags.Compressed) != 0;
            _fileMeta[frame.TaskId] = new FileMeta(meta.Value.name, meta.Value.size, meta.Value.checksum, compressed);
            _eventBus.Publish(new LogEvent("INFO", $"META: {meta.Value.name} ({meta.Value.size} bytes)"));
        }
    }

    private void HandleData(Frame frame, string channel, string peerAddr, string peerName)
    {
        var payload = frame.Payload;
        if ((frame.Flags & FrameFlags.Encrypted) != 0)
        {
            if (!_crypto.HasSessionKey)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"Encrypted BLE chunk without session key (task {frame.TaskId})"));
                return;
            }
            payload = _crypto.Decrypt(payload);
        }

        var ms = _buffers.GetOrAdd(frame.TaskId, _ => new MemoryStream());
        lock (ms)
        {
            ms.Seek(frame.Offset, SeekOrigin.Begin);
            ms.Write(payload, 0, payload.Length);
        }

        if ((frame.Flags & FrameFlags.FinalChunk) != 0)
            Finalize(frame.TaskId, channel, peerAddr, peerName);
    }

    private void Finalize(uint taskId, string channel, string peerAddr, string peerName)
    {
        if (!_buffers.TryRemove(taskId, out var ms)) return;

        byte[] data;
        lock (ms)
        {
            data = ms.ToArray();
        }
        ms.Dispose();

        if (_fileMeta.TryRemove(taskId, out var meta))
        {
            _eventBus.Publish(new FileDataReceivedEvent(taskId, meta.Name, data, meta.Compressed, meta.Checksum, channel, peerAddr, peerName));
        }
        else
        {
            var text = Encoding.UTF8.GetString(data);
            _eventBus.Publish(new TextReceivedEvent(peerAddr, peerName, text));
        }
    }
}
