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

    private StreamSocketListener? _listener;
    private RfcommServiceProvider? _provider;
    private StreamSocket? _clientSocket;

    private readonly ConcurrentDictionary<uint, TransferState> _activeReceives = new();
    private readonly ConcurrentDictionary<uint, FileStream> _openStreams = new();
    private readonly ConcurrentDictionary<uint, TaskCompletionSource<uint>> _pendingResume = new();

    public bool IsActive => _listener != null || _clientSocket != null;

    public RfcommChannel(EventBus eventBus, StorageService storage, CryptoService crypto)
    {
        _eventBus = eventBus;
        _storage = storage;
        _crypto = crypto;
    }

    public async Task<bool> StartServerAsync()
    {
        try
        {
            _provider = await RfcommServiceProvider.CreateAsync(RfcommServiceId.SerialPort);
            _listener = new StreamSocketListener();
            _listener.ConnectionReceived += OnConnectionReceived;
            _provider.StartAdvertising(_listener, true);
            _eventBus.Publish(new LogEvent("INFO", "RFCOMM server started"));
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM server start failed: {ex.Message}"));
            return false;
        }
    }

    public async Task<bool> ConnectToServerAsync(string deviceName)
    {
        StreamSocket? socket = null;
        try
        {
            var selector = RfcommDeviceService.GetDeviceSelector(RfcommServiceId.SerialPort);
            var devices = await DeviceInformation.FindAllAsync(selector);
            var target = devices.FirstOrDefault(d => d.Name.Contains(deviceName, StringComparison.OrdinalIgnoreCase));

            if (target == null && devices.Count > 0)
                target = devices[0];

            if (target == null)
            {
                _eventBus.Publish(new LogEvent("ERROR", "No RFCOMM device found"));
                return false;
            }

            var service = await RfcommDeviceService.FromIdAsync(target.Id);
            if (service == null)
            {
                _eventBus.Publish(new LogEvent("ERROR", "Could not get RFCOMM service"));
                return false;
            }

            socket = new StreamSocket();
            await socket.ConnectAsync(service.ConnectionHostName, service.ConnectionServiceName);
            _clientSocket = socket;
            _ = ReadLoopAsync(_clientSocket);
            _eventBus.Publish(new LogEvent("INFO", $"RFCOMM client connected to {target.Name}"));
            return true;
        }
        catch (Exception ex)
        {
            socket?.Dispose();
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM client connect failed: {ex.Message}"));
            return false;
        }
    }

    private async void OnConnectionReceived(StreamSocketListener sender, StreamSocketListenerConnectionReceivedEventArgs args)
    {
        _eventBus.Publish(new LogEvent("INFO", "RFCOMM client connected"));
        using var socket = args.Socket;
        try
        {
            await ReadLoopAsync(socket);
        }
        finally
        {
            CloseOpenStreams();
            _eventBus.Publish(new LogEvent("INFO", "RFCOMM client disconnected"));
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
            _eventBus.Publish(new LogEvent("WARN", $"RFCOMM read error: {ex.Message}"));
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

        var existing = TransferState.Load(frame.TaskId);
        uint resumeOffset = 0;

        if (existing != null && existing.FileName == name && existing.ReceivedBytes > 0)
        {
            resumeOffset = (uint)existing.ReceivedBytes;
            _eventBus.Publish(new LogEvent("INFO", $"Resuming task {frame.TaskId}: {name} from offset {resumeOffset}"));
            _activeReceives[frame.TaskId] = existing;
        }
        else
        {
            var partialDir = TransferState.GetPartialDir();
            var partialPath = Path.Combine(partialDir, $"{frame.TaskId}_{name}.partial");
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

        _eventBus.Publish(new LogEvent("INFO", $"META: {name} ({size} bytes, resume={resumeOffset})"));
        _eventBus.Publish(new ResumeOffsetEvent(frame.TaskId, resumeOffset));

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
            _eventBus.Publish(new LogEvent("WARN", $"DATA for unknown task {frame.TaskId}"));
            return;
        }

        try
        {
            var payload = frame.Payload;
            if ((frame.Flags & FrameFlags.Encrypted) != 0)
            {
                if (!_crypto.HasSessionKey)
                {
                    _eventBus.Publish(new LogEvent("ERROR", $"Encrypted chunk without session key (task {frame.TaskId})"));
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
            _eventBus.Publish(new LogEvent("ERROR", $"Write chunk failed: {ex.Message}"));
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
            var recvDir = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments),
                "BluetoothTransfer", "recv");
            Directory.CreateDirectory(recvDir);

            var safeName = string.Join("_", state.FileName.Split(Path.GetInvalidFileNameChars()));
            var destPath = Path.Combine(recvDir, safeName);
            if (File.Exists(destPath))
            {
                var nameNoExt = Path.GetFileNameWithoutExtension(safeName);
                var ext = Path.GetExtension(safeName);
                destPath = Path.Combine(recvDir, $"{nameNoExt}_{DateTime.Now:yyyyMMdd_HHmmss}{ext}");
            }

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
                Direction = "recv",
                Type = "file",
                PeerName = "Remote Device",
                PeerAddr = "",
                Name = state.FileName,
                Size = finalSize,
                Status = valid ? "ok" : "failed",
                Channel = "rfcomm",
                Checksum = actualChecksum,
                LocalPath = destPath,
                Note = valid ? "" : "Checksum mismatch"
            });

            _eventBus.Publish(new FileReceivedEvent("", "Remote Device", state.FileName, destPath, finalSize));
            _eventBus.Publish(new LogEvent("INFO", $"File complete: {state.FileName} -> {destPath} (valid={valid})"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Finalize failed: {ex.Message}"));
        }
    }

    public async Task<bool> SendFileAsync(string filePath, uint taskId, string checksum, int chunkSize = 4096,
        bool compress = false, bool encrypt = false, uint startOffset = 0)
    {
        if (_clientSocket == null)
        {
            _eventBus.Publish(new LogEvent("ERROR", "RFCOMM not connected"));
            return false;
        }

        if (encrypt && !_crypto.HasSessionKey)
        {
            _eventBus.Publish(new LogEvent("WARN", "Encryption requested but no session key; sending plaintext"));
            encrypt = false;
        }

        var fileInfo = new FileInfo(filePath);
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
                _eventBus.Publish(new LogEvent("WARN", $"No resume offset from peer for task {taskId}; starting from {offset}"));
            }
            _pendingResume.TryRemove(taskId, out _);

            if (offset > 0)
                _eventBus.Publish(new LogEvent("INFO", $"Resuming transfer from offset {offset}/{totalLen}"));

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

            _eventBus.Publish(new LogEvent("INFO", $"File sent: {fileInfo.Name} ({totalLen} bytes, start={offset})"));
            return true;
        }
        catch (Exception ex)
        {
            _pendingResume.TryRemove(taskId, out _);
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM send failed: {ex.Message}"));
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
            _eventBus.Publish(new LogEvent("WARN", $"Send control frame failed: {ex.Message}"));
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
        _clientSocket?.Dispose();
        _clientSocket = null;
        _provider?.StopAdvertising();
        _provider = null;
        _listener?.Dispose();
        _listener = null;
        _eventBus.Publish(new LogEvent("INFO", "RFCOMM channel closed"));
    }
}
