using System.IO;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class FileTransferService
{
    private readonly EventBus _eventBus;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly RfcommChannel _rfcomm;
    private readonly BleService _ble;
    private readonly CryptoService _crypto;

    public FileTransferService(EventBus eventBus, StorageService storage, AppConfig config,
        RfcommChannel rfcomm, BleService ble, CryptoService crypto)
    {
        _eventBus = eventBus;
        _storage = storage;
        _config = config;
        _rfcomm = rfcomm;
        _ble = ble;
        _crypto = crypto;
        _eventBus.Subscribe<FileDataReceivedEvent>(OnFileDataReceived);
    }

    public async Task<bool> SendFileAsync(string filePath)
    {
        var fileInfo = new FileInfo(filePath);
        if (!fileInfo.Exists)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"文件不存在：{filePath}"));
            return false;
        }

        var taskId = (uint)Random.Shared.Next();
        var useRfcomm = fileInfo.Length > 10240;
        var encrypt = _config.EncryptionEnabled && _crypto.HasSessionKey;

        try
        {
            bool ok;
            string checksum;

            if (useRfcomm)
            {
                checksum = await RfcommChannel.ComputeSha256Async(filePath);
                ok = await _rfcomm.SendFileAsync(filePath, taskId, checksum,
                    _config.RfcommChunkSize, _config.CompressionEnabled, encrypt);
            }
            else
            {
                var fileBytes = await File.ReadAllBytesAsync(filePath);
                checksum = RfcommChannel.ComputeSha256(fileBytes);
                byte[] payload = fileBytes;
                var flags = FrameFlags.None;

                if (_config.CompressionEnabled)
                {
                    payload = RfcommChannel.CompressData(fileBytes);
                    flags |= FrameFlags.Compressed;
                }

                var meta = MetaPayload.Encode("file", fileInfo.Name, fileInfo.Length, checksum, flags);
                var metaOk = await _ble.SendMetaAsync(meta, taskId);
                ok = metaOk && await _ble.SendBinaryChunkedAsync(MsgType.DATA, taskId, payload, flags, encrypt);
            }

            _storage.AddRecord(new TransferRecord
            {
                Direction = TransferConst.DirSend,
                Type = TransferConst.TypeFile,
                PeerName = _ble.ConnectedName ?? "远程设备",
                PeerAddr = _ble.ConnectedAddr ?? "",
                Name = fileInfo.Name,
                Size = fileInfo.Length,
                Status = ok ? TransferConst.StatusOk : TransferConst.StatusFailed,
                Channel = useRfcomm ? TransferConst.ChannelRfcomm : TransferConst.ChannelBle,
                Checksum = checksum,
                Note = ok ? "" : "传输报告失败"
            });

            _eventBus.Publish(new LogEvent(ok ? "INFO" : "ERROR",
                $"文件{(ok ? "已发送" : "发送失败")}：{fileInfo.Name}，通道 {(useRfcomm ? "RFCOMM" : "BLE")}"));
            return ok;
        }
        catch (Exception ex)
        {
            _storage.AddRecord(new TransferRecord
            {
                Direction = TransferConst.DirSend,
                Type = TransferConst.TypeFile,
                PeerName = _ble.ConnectedName ?? "远程设备",
                PeerAddr = _ble.ConnectedAddr ?? "",
                Name = fileInfo.Name,
                Size = fileInfo.Length,
                Status = TransferConst.StatusFailed,
                Channel = useRfcomm ? TransferConst.ChannelRfcomm : TransferConst.ChannelBle,
                Note = ex.Message
            });
            _eventBus.Publish(new LogEvent("ERROR", $"文件发送失败：{ex.Message}"));
            return false;
        }
    }

    private void OnFileDataReceived(FileDataReceivedEvent e)
    {
        try
        {
            ReceiveFile(e.FileName, e.Data, e.Compressed, e.Checksum, e.Channel, e.PeerAddr, e.PeerName);
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"文件接收失败：{ex.Message}"));
        }
    }

    public static string ResolveDestPath(string recvDir, string fileName)
    {
        Directory.CreateDirectory(recvDir);
        var safeName = string.Join("_", fileName.Split(Path.GetInvalidFileNameChars()));
        var destPath = Path.Combine(recvDir, safeName);
        if (File.Exists(destPath))
        {
            var nameNoExt = Path.GetFileNameWithoutExtension(safeName);
            var ext = Path.GetExtension(safeName);
            var stamp = DateTime.Now.ToString("yyyyMMdd_HHmmss");
            destPath = Path.Combine(recvDir, $"{nameNoExt}_{stamp}{ext}");
            var counter = 1;
            while (File.Exists(destPath))
            {
                // 同一秒内到达多个同名文件时追加序号，避免相互覆盖。
                destPath = Path.Combine(recvDir, $"{nameNoExt}_{stamp}_{counter++}{ext}");
            }
        }
        return destPath;
    }

    public string ReceiveFile(string fileName, byte[] data, bool compressed, string checksum,
        string channel, string peerAddr, string peerName)
    {
        var destPath = ResolveDestPath(_config.RecvDirectory, fileName);

        byte[] fileData = compressed ? RfcommChannel.DecompressData(data) : data;

        var actualChecksum = RfcommChannel.ComputeSha256(fileData);
        var valid = string.IsNullOrEmpty(checksum) || actualChecksum == checksum;

        File.WriteAllBytes(destPath, fileData);

        _storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirRecv,
            Type = TransferConst.TypeFile,
            PeerName = peerName,
            PeerAddr = peerAddr,
            Name = fileName,
            Size = fileData.Length,
            Status = valid ? TransferConst.StatusOk : TransferConst.StatusFailed,
            Channel = channel,
            Checksum = actualChecksum,
            LocalPath = destPath,
            Note = valid ? "" : "校验和不匹配"
        });

        _eventBus.Publish(new FileReceivedEvent(peerAddr, peerName, fileName, destPath, fileData.Length));
        _eventBus.Publish(new LogEvent("INFO", $"文件已接收：{fileName} -> {destPath}（校验={(valid ? "通过" : "失败")}）"));
        return destPath;
    }

    public async Task<bool> SendFolderAsync(string folderPath)
    {
        var files = Directory.GetFiles(folderPath, "*", SearchOption.AllDirectories);
        if (files.Length == 0)
        {
            _eventBus.Publish(new LogEvent("WARN", $"文件夹中没有可发送的文件：{folderPath}"));
            return false;
        }

        _eventBus.Publish(new LogEvent("INFO", $"正在发送文件夹：{folderPath}（{files.Length} 个文件）"));

        var allOk = true;
        foreach (var file in files)
        {
            allOk &= await SendFileAsync(file);
        }
        return allOk;
    }
}
