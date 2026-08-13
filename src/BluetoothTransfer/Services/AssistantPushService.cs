using System.IO;
using System.IO.Compression;
using System.Text;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Networking.Sockets;
using BluetoothTransfer.Core.Client;
using BluetoothTransfer.Core.Discovery;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>
/// 接收助手推送服务：与 OPP 相同的发送形态（文件/文本/文件夹），
/// 但走自定义 UUID 的私有分片协议，支持断点续传与 SHA-256 校验。
/// 记录写入 SQLite（channel=assistant）。
/// </summary>
public sealed class AssistantPushService
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;

    public AssistantPushService(EventBus events, StorageService storage, AppConfig config, OppDiscoveryService? discovery = null)
    {
        _events = events ?? throw new ArgumentNullException(nameof(events));
        _storage = storage ?? throw new ArgumentNullException(nameof(storage));
        _config = config ?? throw new ArgumentNullException(nameof(config));
        _discovery = discovery ?? new OppDiscoveryService(events);
    }

    public async Task<bool> SendFileAsync(string deviceAddr, string filePath, bool zip = false, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(filePath) || !File.Exists(filePath))
        {
            _events.Publish(new LogEvent("ERROR", $"文件不存在：{filePath}"));
            return false;
        }
        var info = new FileInfo(filePath);
        if (zip)
        {
            var zipPath = Path.Combine(Path.GetTempPath(), $"bt_asst_zip_{Guid.NewGuid():N}.zip");
            try
            {
                await Task.Run(() =>
                {
                    using var archive = ZipFile.Open(zipPath, ZipArchiveMode.Create);
                    archive.CreateEntryFromFile(filePath, info.Name, CompressionLevel.Optimal);
                }, ct);
                return await SendSourceAsync(deviceAddr, zipPath, info.Name + ".zip", filePath, ct);
            }
            catch (Exception ex)
            {
                _events.Publish(new LogEvent("ERROR", $"文件打包失败：{ex.Message}"));
                return false;
            }
            finally
            {
                TryDelete(zipPath);
            }
        }
        return await SendSourceAsync(deviceAddr, filePath, info.Name, filePath, ct);
    }

    public async Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default)
    {
        var fileName = string.IsNullOrWhiteSpace(name) ? _config.PushTextFileName : name;
        var tempPath = Path.Combine(Path.GetTempPath(), $"bt_asst_text_{Guid.NewGuid():N}.txt");
        try
        {
            await File.WriteAllBytesAsync(tempPath, Encoding.UTF8.GetBytes(text ?? ""), ct);
            return await SendSourceAsync(deviceAddr, tempPath, fileName, tempPath, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"文本临时文件写入失败：{ex.Message}"));
            return false;
        }
        finally
        {
            TryDelete(tempPath);
        }
    }

    public async Task<bool> SendFolderAsync(string deviceAddr, string folderPath, CancellationToken ct = default)
    {
        if (string.IsNullOrWhiteSpace(folderPath) || !Directory.Exists(folderPath))
        {
            _events.Publish(new LogEvent("ERROR", $"文件夹不存在：{folderPath}"));
            return false;
        }
        var folderName = Path.GetFileName(folderPath.TrimEnd('\\', '/'));
        if (string.IsNullOrEmpty(folderName)) folderName = "folder";
        var zipPath = Path.Combine(Path.GetTempPath(), $"bt_asst_folder_{Guid.NewGuid():N}.zip");
        try
        {
            await Task.Run(() => ZipFile.CreateFromDirectory(folderPath, zipPath, CompressionLevel.Optimal, includeBaseDirectory: false), ct);
            return await SendSourceAsync(deviceAddr, zipPath, folderName + ".zip", zipPath, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"文件夹打包失败：{ex.Message}"));
            return false;
        }
        finally
        {
            TryDelete(zipPath);
        }
    }

    private async Task<bool> SendSourceAsync(
        string deviceAddr, string sourcePath, string displayName, string localPathForRecord, CancellationToken ct)
    {
        OppDeviceInfo? device = null;
        try
        {
            device = await _discovery.FindByAddressAsync(deviceAddr, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("WARN", $"OPP 设备发现失败（尝试按已配对设备解析）：{ex.Message}"));
        }
        if (device == null)
        {
            // 接收端只运行助手（不广播 OPP）时，OPP 发现找不到设备，回退到已配对设备列表
            var paired = await AssistantDetector.FindPairedDeviceAsync(deviceAddr, ct);
            if (paired == null)
            {
                var message = $"未找到蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）";
                _events.Publish(new LogEvent("ERROR", message));
                WriteFailedRecord("", deviceAddr, displayName, 0, message);
                return false;
            }
            device = new OppDeviceInfo
            {
                Id = paired.DeviceId,
                Addr = paired.Addr,
                Name = paired.Name,
                IsPaired = true
            };
        }
        if (!device.IsPaired)
        {
            var message = $"设备未配对：{device.Name}（{device.AddrDisplay}），请先在应用内或系统设置中完成配对";
            _events.Publish(new LogEvent("ERROR", message));
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, "设备未配对");
            return false;
        }

        long totalLength;
        string checksum;
        try
        {
            totalLength = new FileInfo(sourcePath).Length;
            checksum = await FileChecksum.ComputeSha256Async(sourcePath);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"计算校验和失败：{ex.Message}"));
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, ex.Message);
            return false;
        }

        // 用内容 SHA-256 作为 transferId：同一文件重发/重试可命中接收端半成品实现断点续传
        var transferId = checksum;
        var retryPolicy = new OppRetryPolicy(_config.OppRetryCount, _config.OppRetryDelaySeconds);
        var attempt = 1;
        string? lastError = null;
        while (true)
        {
            RfcommDeviceService? service = null;
            try
            {
                service = await AssistantDetector.GetAssistantServiceAsync(device.Id, ct);
                if (service == null)
                    throw new AsstProtocolException("对端未运行接收助手（探测不到助手服务）");
                var level = ResolveProtectionLevel(service, _config.OppProtectionLevel);
                var transport = await SocketAsstTransport.ConnectAsync(service, level, ct);
                await using var client = new AssistantClient(transport);
                using var fs = new FileStream(sourcePath, FileMode.Open, FileAccess.Read, FileShare.Read);
                var hello = new AsstHello(transferId, displayName, totalLength, _config.OppChunkSize, checksum);
                var result = await client.SendAsync(hello, (offset, buffer, token) =>
                {
                    fs.Seek(offset, SeekOrigin.Begin);
                    return fs.ReadAsync(buffer, token).AsTask();
                }, (sent, total) => _events.Publish(new TransferProgressEvent(transferId, sent, total, 0)), ct);

                if (!result.Ok)
                {
                    if (result.Error.Contains("忙"))
                    {
                        lastError = "接收端忙（连接重试）";
                        if (retryPolicy.ShouldRetry(attempt, lastError))
                        {
                            var delay = retryPolicy.NextDelay(attempt);
                            _events.Publish(new LogEvent("WARN",
                                $"接收端忙（{displayName}）：{delay.TotalSeconds:0} 秒后第 {attempt + 1}/{retryPolicy.MaxAttempts} 次重试"));
                            attempt++;
                            try
                            {
                                await Task.Delay(delay, ct);
                            }
                            catch (OperationCanceledException)
                            {
                                _events.Publish(new LogEvent("WARN", $"助手推送已取消：{displayName}"));
                                WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消");
                                return false;
                            }
                            continue;
                        }
                        lastError = $"接收端忙（已重试 {attempt - 1} 次）";
                        _events.Publish(new LogEvent("ERROR", $"助手推送失败：{displayName}（{lastError}）"));
                        WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, lastError);
                        return false;
                    }
                    lastError = result.Error;
                    _events.Publish(new LogEvent("ERROR", $"助手推送失败：{displayName}（{result.Error}）"));
                    WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, result.Error);
                    return false;
                }

                _events.Publish(new TransferProgressEvent(transferId, totalLength, totalLength, 0));
                _storage.UpsertDevice(new DeviceInfo
                {
                    Addr = device.Addr,
                    Name = device.Name,
                    LastSeen = DateTime.Now.ToString("o"),
                    LastConnected = DateTime.Now.ToString("o")
                });
                var resumeNote = result.ResumeOffset > 0 ? $"（从偏移 {result.ResumeOffset} 续传）" : "";
                _events.Publish(new LogEvent("INFO",
                    $"助手推送成功：{displayName} -> {device.Name}（{device.AddrDisplay}，{result.BytesSent} 字节{resumeNote}）"));
                WriteOkRecord(device, displayName, totalLength, checksum, localPathForRecord, result.BytesSent, resumeNote);
                return true;
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                _events.Publish(new LogEvent("WARN", $"助手推送已取消：{displayName}"));
                WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消");
                return false;
            }
            catch (Exception ex)
            {
                lastError = string.IsNullOrWhiteSpace(ex.Message)
                    ? $"{ex.GetType().Name} (0x{ex.HResult:X8})"
                    : ex.Message;
            }
            finally
            {
                service?.Dispose();
            }

            if (retryPolicy.ShouldRetry(attempt, lastError))
            {
                var delay = retryPolicy.NextDelay(attempt);
                _events.Publish(new LogEvent("WARN",
                    $"助手推送失败（{displayName}）：{lastError}；{delay.TotalSeconds:0} 秒后第 {attempt + 1}/{retryPolicy.MaxAttempts} 次重试（将从偏移续传）"));
                attempt++;
                try
                {
                    await Task.Delay(delay, ct);
                }
                catch (OperationCanceledException)
                {
                    _events.Publish(new LogEvent("WARN", $"助手推送已取消：{displayName}"));
                    WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消");
                    return false;
                }
                continue;
            }

            var note = attempt > 1 ? $"{lastError}（已重试 {attempt - 1} 次）" : lastError;
            _events.Publish(new LogEvent("ERROR", $"助手推送失败：{displayName}（{note}）"));
            WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, note ?? "未知错误");
            return false;
        }
    }

    private void WriteOkRecord(OppDeviceInfo device, string displayName, long size, string checksum, string localPath, long bytesSent, string resumeNote = "")
    {
        _storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeFile,
            PeerName = device.Name,
            PeerAddr = device.Addr,
            Name = displayName,
            Size = size,
            Status = TransferConst.StatusOk,
            Checksum = checksum,
            Channel = TransferConst.ChannelAssistant,
            LocalPath = localPath,
            Note = $"已发送 {bytesSent} 字节{resumeNote}"
        });
    }

    private void WriteFailedRecord(string peerName, string peerAddr, string displayName, long size, string note)
    {
        _storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeFile,
            PeerName = peerName,
            PeerAddr = peerAddr,
            Name = displayName,
            Size = size,
            Status = TransferConst.StatusFailed,
            Channel = TransferConst.ChannelAssistant,
            Note = note
        });
    }

    /// <summary>
    /// 解析助手通道的连接保护级别，与 OPP 通道共用 <see cref="AppConfig.OppProtectionLevel"/> 配置。
    /// auto：按服务端 SDP 要求的保护级别连接（服务未要求加密则保持明文，与 OPP 路径一致）；
    /// plain：强制明文；encrypt：强制加密认证。
    /// </summary>
    private static SocketProtectionLevel ResolveProtectionLevel(RfcommDeviceService service, string config)
    {
        switch (config?.Trim().ToLowerInvariant())
        {
            case "plain":
                return SocketProtectionLevel.PlainSocket;
            case "encrypt":
                return SocketProtectionLevel.BluetoothEncryptionWithAuthentication;
            default: // auto
                return service.ProtectionLevel != SocketProtectionLevel.PlainSocket
                    ? service.ProtectionLevel
                    : SocketProtectionLevel.PlainSocket;
        }
    }

    private static void TryDelete(string path)
    {
        try
        {
            if (File.Exists(path)) File.Delete(path);
        }
        catch
        {
            // 临时文件清理失败不阻塞流程
        }
    }
}
