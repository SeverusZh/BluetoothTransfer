using System.Collections.Concurrent;
using System.IO;
using System.Security.Cryptography;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Devices.Enumeration;
using Windows.Networking.Sockets;
using Windows.Storage.Streams;

namespace BluetoothTransfer.Services;

public class RfcommChannel
{
    private readonly EventBus _eventBus;
    private StreamSocketListener? _listener;
    private RfcommServiceProvider? _provider;
    private StreamSocket? _clientSocket;

    public bool IsActive => _listener != null || _clientSocket != null;

    public RfcommChannel(EventBus eventBus)
    {
        _eventBus = eventBus;
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

            _clientSocket = new StreamSocket();
            await _clientSocket.ConnectAsync(service.ConnectionHostName, service.ConnectionServiceName);
            _eventBus.Publish(new LogEvent("INFO", $"RFCOMM client connected to {target.Name}"));
            return true;
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"RFCOMM client connect failed: {ex.Message}"));
            return false;
        }
    }

    private async void OnConnectionReceived(StreamSocketListener sender, StreamSocketListenerConnectionReceivedEventArgs args)
    {
        _eventBus.Publish(new LogEvent("INFO", "RFCOMM client connected"));
        var socket = args.Socket;
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
                    ProcessFrame(frame);
            }
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("WARN", $"RFCOMM read error: {ex.Message}"));
        }
    }

    private readonly ConcurrentDictionary<uint, TransferState> _activeReceives = new();

    private void ProcessFrame(Frame frame)
    {
        switch (frame.MsgType)
        {
            case MsgType.META:
                HandleMeta(frame);
                break;
            case MsgType.DATA:
                HandleData(frame);
                break;
            case MsgType.END:
                HandleEnd(frame);
                break;
        }
    }

    private void HandleMeta(Frame frame)
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
            using var fs = new FileStream(state.PartialPath, FileMode.OpenOrCreate, FileAccess.Write);
            fs.Seek(frame.Offset, SeekOrigin.Begin);
            fs.Write(frame.Payload, 0, frame.Payload.Length);

            state.ReceivedBytes = frame.Offset + frame.Payload.Length;
            state.Save();

            _eventBus.Publish(new TransferProgressEvent(frame.TaskId.ToString(), (long)state.ReceivedBytes, state.TotalSize, 0));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Write chunk failed: {ex.Message}"));
        }
    }

    private void HandleEnd(Frame frame)
    {
        if (!_activeReceives.TryGetValue(frame.TaskId, out var state))
            return;

        _activeReceives.TryRemove(frame.TaskId, out _);

        try
        {
            var data = File.ReadAllBytes(state.PartialPath);
            byte[] fileData = state.Compressed ? DecompressData(data) : data;
            var actualChecksum = ComputeSha256(fileData);
            var valid = string.IsNullOrEmpty(state.Checksum) || actualChecksum == state.Checksum;

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

            File.WriteAllBytes(destPath, fileData);
            state.Delete();

            _eventBus.Publish(new FileReceivedEvent("", "Remote Device", state.FileName, destPath, fileData.Length));
            _eventBus.Publish(new LogEvent("INFO", $"File complete: {state.FileName} -> {destPath} (valid={valid})"));
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Finalize failed: {ex.Message}"));
        }
    }

    public async Task SendFileAsync(string filePath, uint taskId, int chunkSize = 4096, bool compress = false, uint startOffset = 0)
    {
        if (_clientSocket == null)
        {
            _eventBus.Publish(new LogEvent("ERROR", "RFCOMM not connected"));
            return;
        }

        var fileInfo = new FileInfo(filePath);
        byte[] payload;

        if (compress)
        {
            var fileBytes = await File.ReadAllBytesAsync(filePath);
            payload = CompressData(fileBytes);
        }
        else
        {
            payload = await File.ReadAllBytesAsync(filePath);
        }

        var checksum = ComputeSha256(await File.ReadAllBytesAsync(filePath));
        var flags = compress ? FrameFlags.Compressed : FrameFlags.None;
        var totalLen = (uint)payload.Length;

        var meta = MetaPayload.Encode("file", fileInfo.Name, (long)totalLen, checksum, flags);

        using var writer = new DataWriter(_clientSocket.OutputStream);

        WriteFrame(writer, new Frame
        {
            MsgType = MsgType.META,
            TaskId = taskId,
            TotalLen = totalLen,
            Offset = startOffset,
            Flags = flags,
            Payload = meta
        });
        await writer.StoreAsync();

        ushort seq = 0;
        uint offset = startOffset;

        if (startOffset > 0)
            _eventBus.Publish(new LogEvent("INFO", $"Resuming transfer from offset {startOffset}/{totalLen}"));

        while (offset < totalLen)
        {
            var len = (int)Math.Min(chunkSize, totalLen - offset);
            var chunk = new byte[len];
            Array.Copy(payload, offset, chunk, 0, len);

            var isFinal = offset + len >= totalLen;
            WriteFrame(writer, new Frame
            {
                MsgType = MsgType.DATA,
                TaskId = taskId,
                SeqNo = seq++,
                TotalLen = totalLen,
                Offset = offset,
                Flags = isFinal ? FrameFlags.FinalChunk : FrameFlags.None,
                Payload = chunk
            });
            await writer.StoreAsync();

            offset += (uint)len;
            _eventBus.Publish(new TransferProgressEvent(taskId.ToString(), offset, totalLen, 0));
        }

        WriteFrame(writer, new Frame
        {
            MsgType = MsgType.END,
            TaskId = taskId,
            TotalLen = totalLen,
            Offset = totalLen,
            Flags = FrameFlags.FinalChunk,
            Payload = Array.Empty<byte>()
        });
        await writer.StoreAsync();

        _eventBus.Publish(new LogEvent("INFO", $"File sent: {fileInfo.Name} ({totalLen} bytes, start={startOffset})"));
    }

    private static void WriteFrame(DataWriter writer, Frame frame)
    {
        var frameBytes = frame.Serialize();
        writer.WriteUInt32((uint)frameBytes.Length);
        writer.WriteBytes(frameBytes);
    }

    private async Task SendFrameAsync(Frame frame)
    {
        if (_clientSocket == null) return;
        using var writer = new DataWriter(_clientSocket.OutputStream);
        WriteFrame(writer, frame);
        await writer.StoreAsync();
    }

    public static byte[] CompressData(byte[] data)
    {
        using var output = new MemoryStream();
        using (var deflate = new System.IO.Compression.DeflateStream(output, System.IO.Compression.CompressionLevel.Optimal))
        {
            deflate.Write(data, 0, data.Length);
        }
        return output.ToArray();
    }

    public static byte[] DecompressData(byte[] data)
    {
        using var input = new MemoryStream(data);
        using var deflate = new System.IO.Compression.DeflateStream(input, System.IO.Compression.CompressionMode.Decompress);
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
        _clientSocket?.Dispose();
        _clientSocket = null;
        _provider?.StopAdvertising();
        _provider = null;
        _listener?.Dispose();
        _listener = null;
        _eventBus.Publish(new LogEvent("INFO", "RFCOMM channel closed"));
    }
}
