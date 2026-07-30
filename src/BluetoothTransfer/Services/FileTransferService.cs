using System.IO;
using System.Security.Cryptography;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class FileTransferService
{
    private readonly EventBus _eventBus;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly RfcommChannel _rfcomm;
    private readonly BleService _ble;

    public FileTransferService(EventBus eventBus, StorageService storage, AppConfig config,
        RfcommChannel rfcomm, BleService ble)
    {
        _eventBus = eventBus;
        _storage = storage;
        _config = config;
        _rfcomm = rfcomm;
        _ble = ble;
    }

    public async Task<bool> SendFileAsync(string filePath)
    {
        var fileInfo = new FileInfo(filePath);
        if (!fileInfo.Exists)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"File not found: {filePath}"));
            return false;
        }

        var taskId = (uint)Random.Shared.Next();
        var useRfcomm = fileInfo.Length > 10240;

        try
        {
            if (useRfcomm)
            {
                await _rfcomm.SendFileAsync(filePath, taskId, _config.RfcommChunkSize, _config.CompressionEnabled);
            }
            else
            {
                var fileBytes = await File.ReadAllBytesAsync(filePath);
                var checksum = RfcommChannel.ComputeSha256(fileBytes);
                byte[] payload = fileBytes;
                var flags = FrameFlags.None;

                if (_config.CompressionEnabled)
                {
                    payload = RfcommChannel.CompressData(fileBytes);
                    flags |= FrameFlags.Compressed;
                }

                var meta = MetaPayload.Encode("file", fileInfo.Name, fileInfo.Length, checksum, flags);
                await _ble.SendMetaAsync(meta);
                await _ble.SendBinaryChunkedAsync(MsgType.DATA, taskId, payload, flags);
            }

            _storage.AddRecord(new TransferRecord
            {
                Direction = "send",
                Type = "file",
                PeerName = _ble.ConnectedName ?? "remote",
                PeerAddr = _ble.ConnectedAddr ?? "",
                Name = fileInfo.Name,
                Size = fileInfo.Length,
                Status = "ok",
                Channel = useRfcomm ? "rfcomm" : "ble",
                Checksum = RfcommChannel.ComputeSha256(await File.ReadAllBytesAsync(filePath))
            });

            _eventBus.Publish(new LogEvent("INFO", $"File sent: {fileInfo.Name} via {(useRfcomm ? "RFCOMM" : "BLE")}"));
            return true;
        }
        catch (Exception ex)
        {
            _storage.AddRecord(new TransferRecord
            {
                Direction = "send",
                Type = "file",
                PeerName = _ble.ConnectedName ?? "remote",
                PeerAddr = _ble.ConnectedAddr ?? "",
                Name = fileInfo.Name,
                Size = fileInfo.Length,
                Status = "failed",
                Channel = useRfcomm ? "rfcomm" : "ble",
                Note = ex.Message
            });
            _eventBus.Publish(new LogEvent("ERROR", $"File send failed: {ex.Message}"));
            return false;
        }
    }

    public string ReceiveFile(string fileName, byte[] data, bool compressed, string checksum)
    {
        var recvDir = _config.RecvDirectory;
        Directory.CreateDirectory(recvDir);

        byte[] fileData = compressed ? RfcommChannel.DecompressData(data) : data;

        var actualChecksum = RfcommChannel.ComputeSha256(fileData);
        var valid = string.IsNullOrEmpty(checksum) || actualChecksum == checksum;

        var safeName = string.Join("_", fileName.Split(Path.GetInvalidFileNameChars()));
        var destPath = Path.Combine(recvDir, safeName);

        if (File.Exists(destPath))
        {
            var nameNoExt = Path.GetFileNameWithoutExtension(safeName);
            var ext = Path.GetExtension(safeName);
            destPath = Path.Combine(recvDir, $"{nameNoExt}_{DateTime.Now:yyyyMMdd_HHmmss}{ext}");
        }

        File.WriteAllBytes(destPath, fileData);

        _storage.AddRecord(new TransferRecord
        {
            Direction = "recv",
            Type = "file",
            PeerName = "remote",
            PeerAddr = "",
            Name = fileName,
            Size = fileData.Length,
            Status = valid ? "ok" : "failed",
            Channel = "rfcomm",
            Checksum = actualChecksum,
            LocalPath = destPath,
            Note = valid ? "" : "Checksum mismatch"
        });

        _eventBus.Publish(new FileReceivedEvent("", "Remote Device", fileName, destPath, fileData.Length));
        _eventBus.Publish(new LogEvent("INFO", $"File received: {fileName} -> {destPath} (valid={valid})"));
        return destPath;
    }

    public async Task SendFolderAsync(string folderPath)
    {
        var files = Directory.GetFiles(folderPath, "*", SearchOption.AllDirectories);
        _eventBus.Publish(new LogEvent("INFO", $"Sending folder: {folderPath} ({files.Length} files)"));

        foreach (var file in files)
        {
            await SendFileAsync(file);
        }
    }
}
