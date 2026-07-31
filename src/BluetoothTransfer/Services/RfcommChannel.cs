using System.Collections.Concurrent;
using System.IO;
using System.IO.Compression;
using System.Security.Cryptography;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Devices.Enumeration;
using Windows.Networking.Sockets;
using Windows.Storage.Streams;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class RfcommChannel
{
    private readonly EventBus _eventBus;
    private readonly StorageService _storage;
    private readonly CryptoService _crypto;
    private readonly AppConfig _config;

    private StreamSocketListener? _listener;
    private RfcommServiceProvider? _provider;
    private StreamSocket? _clientSocket;

    private readonly ConcurrentDictionary<uint, TransferState> _activeReceives = new();
    private readonly ConcurrentDictionary<uint, FileStream> _openStreams = new();
    private readonly ConcurrentDictionary<uint, TaskCompletionSource<uint>> _pendingResume = new();

    /// <summary>单帧长度上限（RFCOMM 分块为 4KB，留足余量防止对端异常帧导致内存暴涨）。</summary>
    private const uint MaxFrameSize = 512 * 1024;

    public bool IsActive => _listener != null || _clientSocket != null;

    public RfcommChannel(EventBus eventBus, StorageService storage, CryptoService crypto, AppConfig config)
    {
        _eventBus = eventBus;
        _storage = storage;
        _crypto = crypto;
        _config = config;
    }

    public async Task<bool> StartServerAsync()
    {
        try
        {
            _provider = await RfcommServiceProvider.CreateAsync(RfcommServiceId.SerialPort);
            _listener = new StreamSocketListener();
            _listener.ConnectionReceived += OnConnectionReceived;
            _provider.StartAdvertising(_listener, true);
            _eventBus.Publish(new LogEvent("INFO", "RFCOMM 服务端已启动"));
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM 服务端启动失败：{ex.Message}"));
            return false;
        }
    }

    public async Task<bool> ConnectToServerAsync(string deviceName)
    {
        StreamSocket? socket = null;
        RfcommDeviceService? service = null;
        try
        {
            var selector = RfcommDeviceService.GetDeviceSelector(RfcommServiceId.SerialPort);
            var devices = await DeviceInformation.FindAllAsync(selector);
            var target = devices.FirstOrDefault(d => d.Name.Contains(deviceName, StringComparison.OrdinalIgnoreCase));

            if (target == null && devices.Count > 0)
                target = devices[0];

            if (target == null)
            {
                _eventBus.Publish(new LogEvent("ERROR", "未找到 RFCOMM 设备"));
                return false;
            }

            service = await RfcommDeviceService.FromIdAsync(target.Id);
            if (service == null)
            {
                _eventBus.Publish(new LogEvent("ERROR", "无法获取 RFCOMM 服务"));
                return false;
            }

            socket = new StreamSocket();
            await socket.ConnectAsync(service.ConnectionHostName, service.ConnectionServiceName);
            _clientSocket = socket;
            _ = ReadLoopAsync(_clientSocket);
            _eventBus.Publish(new LogEvent("INFO", $"RFCOMM 已连接到 {target.Name}"));
            return true;
        }
        catch (Exception ex)
        {
            socket?.Dispose();
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM 连接失败：{ex.Message}"));
            return false;
        }
        finally
        {
            service?.Dispose();
        }
    }

    private async void OnConnectionReceived(StreamSocketListener sender, StreamSocketListenerConnectionReceivedEventArgs args)
    {
        _eventBus.Publish(new LogEvent("INFO", "RFCOMM 对端已连接"));
        using var socket = args.Socket;
        try
        {
            await ReadLoopAsync(socket);
        }
        finally
        {
            CloseOpenStreams();
            _eventBus.Publish(new LogEvent("INFO", "RFCOMM 对端已断开"));
        }
    }

    private async Task ReadLoopAsync(StreamSocket socket)
    {
        try
        {
            using var reader = new DataReader(socket.InputStream);
            reader.InputStreamOptions = InputStreamOptions.Partial;

            while (true)
            {
                uint bytesRead = await reader.LoadAsync(4);
                if (bytesRead < 4) break;
                uint frameLen = reader.ReadUInt32();
                if (frameLen == 0 || frameLen > MaxFrameSize)
                {
                    _eventBus.Publish(new LogEvent("WARN", $"帧长度异常（{frameLen}），终止读取"));
                    break;
                }

                bytesRead = await reader.LoadAsync(frameLen);
                if (bytesRead < frameLen) break;

                var frameData = new byte[frameLen];
                reader.ReadBytes(frameData);

                var frame = Frame.Deserialize(frameData);
                if (frame != null)
                    ProcessFrame(frame, socket);
            }
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"RFCOMM 读取错误：{ex.Message}"));
        }
    }

    private void ProcessFrame(Frame frame, StreamSocket socket)
    {
        switch (frame.MsgType)
        {
            case MsgType.META:
                HandleMeta(frame, socket);
                break;
            case MsgType.DATA:
                HandleData(frame);
                break;
            case MsgType.END:
                HandleEnd(frame);
                break;
            case MsgType.ACK:
                HandleAck(frame);
                break;
            default:
                _eventBus.Publish(new LogEvent("WARN", $"未知消息类型 {frame.MsgType}"));
                break;
        }
    }

    private void HandleAck(Frame frame)
    {
        if (_pendingResume.TryRemove(frame.TaskId, out var tcs))
            tcs.TrySetResult(frame.Offset);
    }

    private void HandleMeta(Frame frame, StreamSocket socket)
    {
        var meta = MetaPayload.Decode(frame.Payload);
        if (meta == null) return;

        var (type, name, size, checksum) = meta.Value;
        var compressed = (frame.Flags & FrameFlags.Compressed) != 0;

        uint resumeOffset = 0;
        var existing = TransferState.Load(frame.TaskId);
        if (existing != null && existing.FileName == name && existing.ReceivedBytes > 0)
        {
            var partial = new FileInfo(existing.PartialPath);
            if (partial.Exists && partial.Length >= existing.ReceivedBytes)
            {
                resumeOffset = (uint)existing.ReceivedBytes;
                _eventBus.Publish(new LogEvent("INFO", $"续传任务 {frame.TaskId}：{name}，偏移 {resumeOffset}"));
                _activeReceives[frame.TaskId] = existing;
            }
            else
            {
                // 状态记录存在但部分文件缺失/长度不足（例如上次残留或手动清理），
                // 直接续传会写出带空洞的损坏文件，须从 0 重新接收。
                _eventBus.Publish(new LogEvent("WARN", $"续传状态无效（任务 {frame.TaskId}，partial 文件缺失），将从 0 重新接收"));
                try { File.Delete(existing.PartialPath); } catch { }
                existing = null;
            }
        }

        if (existing == null)
        {
            var partialDir = TransferState.GetPartialDir();
            var partialPath = Path.Combine(partialDir, $"{frame.TaskId}_{name}.partial");
            // 清理可能残留的旧 partial 文件，避免新传输在文件末尾带上过期数据。
            try { File.Delete(partialPath); } catch { }
            var state = new TransferState
            {
                TaskId = frame.TaskId,
                FileName = name,
                PartialPath = partialPath,
                TotalSize = size,
                ReceivedBytes = 0,
                Checksum = checksum,
                Compressed = compressed
            };
            state.Save();
            _activeReceives[frame.TaskId] = state;
        }

        _eventBus.Publish(new LogEvent("INFO", $"元数据：{name}（{size} 字节，续传偏移={resumeOffset}）"));

        _ = SendFrameOnSocketAsync(socket, new Frame
        {
            MsgType = MsgType.ACK,
            TaskId = frame.TaskId,
            Offset = resumeOffset,
            Flags = FrameFlags.None,
            Payload = Array.Empty<byte>()
        });
    }

    private void HandleData(Frame frame)
    {
        if (!_activeReceives.TryGetValue(frame.TaskId, out var state))
        {
            _eventBus.Publish(new LogEvent("WARN", $"收到未知任务 {frame.TaskId} 的数据"));
            return;
        }

        try
        {
            var payload = frame.Payload;
            if ((frame.Flags & FrameFlags.Encrypted) != 0)
            {
                if (!_crypto.HasSessionKey)
                {
                    _eventBus.Publish(new LogEvent("ERROR", $"收到加密分片但无会话密钥（任务 {frame.TaskId}）"));
                    return;
                }
                payload = _crypto.Decrypt(payload);
            }

            var fs = _openStreams.GetOrAdd(frame.TaskId,
                _ => new FileStream(state.PartialPath, FileMode.OpenOrCreate, FileAccess.Write, FileShare.None));
            fs.Seek(frame.Offset, SeekOrigin.Begin);
            fs.Write(payload, 0, payload.Length);
            fs.Flush();

            state.ReceivedBytes = Math.Max(state.ReceivedBytes, (long)frame.Offset + payload.Length);
            state.Save();

            _eventBus.Publish(new TransferProgressEvent(frame.TaskId.ToString(), state.ReceivedBytes, state.TotalSize, 0));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"写入分片失败：{ex.Message}"));
        }
    }

    private void HandleEnd(Frame frame)
    {
        if (!_activeReceives.TryRemove(frame.TaskId, out var state))
            return;

        if (_openStreams.TryRemove(frame.TaskId, out var openFs))
        {
            try { openFs.Flush(); openFs.Dispose(); } catch { }
        }

        try
        {
            var recvDir = _config.RecvDirectory;
            var destPath = FileTransferService.ResolveDestPath(recvDir, state.FileName);

            string actualChecksum;
            long finalSize;
            using (var src = new FileStream(state.PartialPath, FileMode.Open, FileAccess.Read, FileShare.None))
            using (var source = state.Compressed
                ? new DeflateStream(src, CompressionMode.Decompress)
                : (Stream)src)
            using (var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA256))
            using (var dst = new FileStream(destPath, FileMode.Create, FileAccess.Write, FileShare.None))
            {
                var buffer = new byte[81920];
                int read;
                long total = 0;
                while ((read = source.Read(buffer, 0, buffer.Length)) > 0)
                {
                    sha.AppendData(buffer, 0, read);
                    dst.Write(buffer, 0, read);
                    total += read;
                }
                actualChecksum = Convert.ToHexString(sha.GetHashAndReset());
                finalSize = total;
            }

            var valid = string.IsNullOrEmpty(state.Checksum) || actualChecksum == state.Checksum;
            state.Delete();

            _storage.AddRecord(new TransferRecord
            {
                Direction = TransferConst.DirRecv,
                Type = TransferConst.TypeFile,
                PeerName = "远程设备",
                PeerAddr = "",
                Name = state.FileName,
                Size = finalSize,
                Status = valid ? TransferConst.StatusOk : TransferConst.StatusFailed,
                Channel = TransferConst.ChannelRfcomm,
                Checksum = actualChecksum,
                LocalPath = destPath,
                Note = valid ? "" : "校验和不匹配"
            });

            _eventBus.Publish(new FileReceivedEvent("", "远程设备", state.FileName, destPath, finalSize));
            _eventBus.Publish(new LogEvent("INFO", $"文件接收完成：{state.FileName} -> {destPath} (校验={(valid ? "通过" : "失败")})"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"文件落盘失败：{ex.Message}"));
        }
    }

    public async Task<bool> SendFileAsync(string filePath, uint taskId, string checksum, int chunkSize = 4096,
        bool compress = false, bool encrypt = false, uint startOffset = 0)
    {
        if (_clientSocket == null)
        {
            _eventBus.Publish(new LogEvent("ERROR", "RFCOMM 未连接"));
            return false;
        }

        if (encrypt && !_crypto.HasSessionKey)
        {
            _eventBus.Publish(new LogEvent("WARN", "请求加密但无会话密钥，将以明文发送"));
            encrypt = false;
        }

        var fileInfo = new FileInfo(filePath);
        if (fileInfo.Length > uint.MaxValue)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"文件过大（{fileInfo.Length} 字节），超出协议单任务上限"));
            return false;
        }

        string? tempCompressed = null;

        try
        {
            string dataSource = filePath;
            long totalLen;
            var flags = FrameFlags.None;
            if (compress)
            {
                tempCompressed = Path.Combine(TransferState.GetPartialDir(), $"send_{taskId}.deflate");
                await CompressFileAsync(filePath, tempCompressed);
                dataSource = tempCompressed;
                totalLen = new FileInfo(tempCompressed).Length;
                flags |= FrameFlags.Compressed;
            }
            else
            {
                totalLen = fileInfo.Length;
            }

            var resumeTcs = new TaskCompletionSource<uint>(TaskCreationOptions.RunContinuationsAsynchronously);
            _pendingResume[taskId] = resumeTcs;

            using var writer = new DataWriter(_clientSocket.OutputStream);

            WriteFrame(writer, new Frame
            {
                MsgType = MsgType.META,
                TaskId = taskId,
                TotalLen = (uint)totalLen,
                Offset = startOffset,
                Flags = flags,
                Payload = MetaPayload.Encode("file", fileInfo.Name, totalLen, checksum, flags)
            });
            await writer.StoreAsync();

            uint offset = startOffset;
            var completed = await Task.WhenAny(resumeTcs.Task, Task.Delay(3000));
            if (completed == resumeTcs.Task)
            {
                var peerOffset = await resumeTcs.Task;
                if (peerOffset > offset && peerOffset <= totalLen) offset = peerOffset;
            }
            else
            {
                _eventBus.Publish(new LogEvent("WARN", $"任务 {taskId} 未收到对端续传偏移，从 {offset} 开始"));
            }
            _pendingResume.TryRemove(taskId, out _);

            if (offset > 0)
                _eventBus.Publish(new LogEvent("INFO", $"从偏移 {offset}/{totalLen} 续传"));

            ushort seq = 0;
            using (var fs = new FileStream(dataSource, FileMode.Open, FileAccess.Read, FileShare.None))
            {
                fs.Seek(offset, SeekOrigin.Begin);
                var buffer = new byte[chunkSize];
                long pos = offset;
                int read;
                while (pos < totalLen &&
                       (read = await fs.ReadAsync(buffer, 0, (int)Math.Min(chunkSize, totalLen - pos))) > 0)
                {
                    var chunk = new byte[read];
                    Array.Copy(buffer, chunk, read);

                    byte[] payload = chunk;
                    var dataFlags = FrameFlags.None;
                    if (encrypt)
                    {
                        payload = _crypto.Encrypt(chunk);
                        dataFlags |= FrameFlags.Encrypted;
                    }
                    if (pos + read >= totalLen)
                        dataFlags |= FrameFlags.FinalChunk;

                    WriteFrame(writer, new Frame
                    {
                        MsgType = MsgType.DATA,
                        TaskId = taskId,
                        SeqNo = seq++,
                        TotalLen = (uint)totalLen,
                        Offset = (uint)pos,
                        Flags = dataFlags,
                        Payload = payload
                    });
                    await writer.StoreAsync();

                    pos += read;
                    _eventBus.Publish(new TransferProgressEvent(taskId.ToString(), pos, totalLen, 0));
                }
            }

            WriteFrame(writer, new Frame
            {
                MsgType = MsgType.END,
                TaskId = taskId,
                TotalLen = (uint)totalLen,
                Offset = (uint)totalLen,
                Flags = FrameFlags.FinalChunk,
                Payload = Array.Empty<byte>()
            });
            await writer.StoreAsync();

            _eventBus.Publish(new LogEvent("INFO", $"文件已发送：{fileInfo.Name}（{totalLen} 字节，起始={offset}）"));
            return true;
        }
        catch (Exception ex)
        {
            _pendingResume.TryRemove(taskId, out _);
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM 发送失败：{ex.Message}"));
            return false;
        }
        finally
        {
            if (tempCompressed != null)
            {
                try { File.Delete(tempCompressed); } catch { }
            }
        }
    }

    private async Task SendFrameOnSocketAsync(StreamSocket socket, Frame frame)
    {
        try
        {
            using var writer = new DataWriter(socket.OutputStream);
            WriteFrame(writer, frame);
            await writer.StoreAsync();
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"发送控制帧失败：{ex.Message}"));
        }
    }

    private static void WriteFrame(DataWriter writer, Frame frame)
    {
        var frameBytes = frame.Serialize();
        writer.WriteUInt32((uint)frameBytes.Length);
        writer.WriteBytes(frameBytes);
    }

    private void CloseOpenStreams()
    {
        foreach (var kvp in _openStreams)
        {
            if (_openStreams.TryRemove(kvp.Key, out var fs))
            {
                try { fs.Flush(); fs.Dispose(); } catch { }
            }
        }
    }

    public static async Task<string> ComputeSha256Async(string path)
    {
        using var sha = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
        using var fs = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        var buffer = new byte[81920];
        int read;
        while ((read = await fs.ReadAsync(buffer, 0, buffer.Length)) > 0)
            sha.AppendData(buffer, 0, read);
        return Convert.ToHexString(sha.GetHashAndReset());
    }

    private static async Task CompressFileAsync(string src, string dst)
    {
        using var input = new FileStream(src, FileMode.Open, FileAccess.Read, FileShare.Read);
        using var output = new FileStream(dst, FileMode.Create, FileAccess.Write, FileShare.None);
        using var deflate = new DeflateStream(output, CompressionLevel.Optimal);
        await input.CopyToAsync(deflate);
    }

    public static byte[] CompressData(byte[] data)
    {
        using var output = new MemoryStream();
        using (var deflate = new DeflateStream(output, CompressionLevel.Optimal))
        {
            deflate.Write(data, 0, data.Length);
        }
        return output.ToArray();
    }

    public static byte[] DecompressData(byte[] data)
    {
        using var input = new MemoryStream(data);
        using var deflate = new DeflateStream(input, CompressionMode.Decompress);
        using var output = new MemoryStream();
        deflate.CopyTo(output);
        return output.ToArray();
    }

    public static string ComputeSha256(byte[] data)
    {
        return Convert.ToHexString(SHA256.HashData(data));
    }

    public void Close()
    {
        CloseOpenStreams();
        _activeReceives.Clear();
        _pendingResume.Clear();
        _clientSocket?.Dispose();
        _clientSocket = null;
        _provider?.StopAdvertising();
        _provider = null;
        _listener?.Dispose();
        _listener = null;
        _eventBus.Publish(new LogEvent("INFO", "RFCOMM 通道已关闭"));
    }
}
