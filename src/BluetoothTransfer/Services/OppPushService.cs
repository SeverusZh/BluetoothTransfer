using System.IO;
using System.IO.Compression;
using System.Text;
using Windows.Devices.Bluetooth;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Storage.Streams;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>
/// OPP 通用推送服务：仅发送端运行本应用，向支持"蓝牙文件接收"的任意设备推送文件。
/// 支持单文件、文本（转 .txt）、文件夹（转 .zip），记录写入 SQLite（channel=opp）。
/// </summary>
public class OppPushService
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;

    public OppPushService(EventBus events, StorageService storage, AppConfig config, OppDiscoveryService? discovery = null)
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
        if (info.Length > uint.MaxValue)
        {
            _events.Publish(new LogEvent("ERROR", $"文件超过 4GB 上限：{info.Name}"));
            return false;
        }
        if (zip)
        {
            var zipPath = Path.Combine(Path.GetTempPath(), $"bt_opp_zip_{Guid.NewGuid():N}.zip");
            try
            {
                await Task.Run(() =>
                {
                    using var archive = ZipFile.Open(zipPath, ZipArchiveMode.Create);
                    archive.CreateEntryFromFile(filePath, info.Name, CompressionLevel.Optimal);
                }, ct);
                return await SendSourceAsync(deviceAddr, zipPath, info.Name + ".zip", "application/zip", filePath, ct);
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
        return await SendSourceAsync(deviceAddr, filePath, info.Name, MimeForName(info.Name), filePath, ct);
    }

    public async Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default)
    {
        var fileName = string.IsNullOrWhiteSpace(name) ? _config.PushTextFileName : name;
        var tempPath = Path.Combine(Path.GetTempPath(), $"bt_opp_text_{Guid.NewGuid():N}.txt");
        try
        {
            await File.WriteAllBytesAsync(tempPath, Encoding.UTF8.GetBytes(text ?? ""), ct);
            return await SendSourceAsync(deviceAddr, tempPath, fileName, "text/plain", tempPath, ct);
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
        var zipPath = Path.Combine(Path.GetTempPath(), $"bt_opp_folder_{Guid.NewGuid():N}.zip");
        try
        {
            await Task.Run(() => ZipFile.CreateFromDirectory(folderPath, zipPath, CompressionLevel.Optimal, includeBaseDirectory: false), ct);
            var zipName = folderName + ".zip";
            return await SendSourceAsync(deviceAddr, zipPath, zipName, "application/zip", zipPath, ct);
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

    /// <summary>按地址解析设备（测试可通过子类覆写注入假设备）。</summary>
    protected virtual async Task<OppDeviceInfo> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
    {
        var device = await _discovery.FindByAddressAsync(deviceAddr, ct);
        if (device == null)
            throw new ObexException($"未找到支持 OPP 的蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）");
        return device;
    }

    /// <summary>打开 OPP 传输（测试可通过子类覆写注入假传输）。</summary>
    protected virtual Task<IObexTransport> OpenTransportAsync(OppDeviceInfo device, CancellationToken ct)
        => OpenSocketTransportAsync(device.Id, ct);

    private async Task<IObexTransport> OpenSocketTransportAsync(string deviceId, CancellationToken ct)
    {
        RfcommDeviceService? service = null;
        try
        {
            service = await RfcommDeviceService.FromIdAsync(deviceId).AsTask(ct);
            if (service == null)
                throw new ObexException("无法获取设备的 OPP 服务，请确认设备已配对");
            await EnsureOppServiceAsync(service);
            var transport = await SocketObexTransport.ConnectAsync(service, _config.OppProtectionLevel, ct);
            _events.Publish(new LogEvent("DEBUG",
                $"OPP 连接已建立，保护级别 {transport.ProtectionLevel}（配置 {_config.OppProtectionLevel}）"));
            return transport;
        }
        finally
        {
            service?.Dispose();
        }
    }

    /// <summary>
    /// SDP 预检：确认服务声明了 OPP（0x1105）并记录保护级别/服务版本。
    /// SDP 读取失败或解析不确定时不拒绝（部分蓝牙栈不返回属性，以连接结果为准）。
    /// </summary>
    private async Task EnsureOppServiceAsync(RfcommDeviceService service)
    {
        try
        {
            _events.Publish(new LogEvent("DEBUG",
                $"OPP 服务保护级别：required={service.ProtectionLevel} max={service.MaxProtectionLevel}"));
            var attrs = await service.GetSdpRawAttributesAsync(BluetoothCacheMode.Uncached);
            if (attrs == null || attrs.Count == 0)
                return;

            if (attrs.TryGetValue(0x0100, out var classList) && classList != null)
            {
                var payload = ReadAllBytes(classList);
                var (foundOpp, sawUuid) = ScanServiceClassList(payload);
                if (sawUuid && !foundOpp)
                    throw new ObexException("设备服务 SDP 未声明 OPP（0x1105），不支持蓝牙文件接收");
            }

            if (attrs.TryGetValue(0x0300, out var version) && version != null)
            {
                var v = ReadAllBytes(version);
                if (v.Length >= 4)
                    _events.Publish(new LogEvent("DEBUG", $"OPP 服务版本属性：0x{v[^4]:X2}{v[^3]:X2}{v[^2]:X2}{v[^1]:X2}"));
            }
        }
        catch (ObexException)
        {
            throw;
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("WARN", $"OPP SDP 预检未完成（继续尝试连接）：{ex.Message}"));
        }
    }

    private static byte[] ReadAllBytes(IBuffer buffer)
    {
        using var reader = DataReader.FromBuffer(buffer);
        var data = new byte[reader.UnconsumedBufferLength];
        reader.ReadBytes(data);
        return data;
    }

    /// <summary>
    /// 扫描 ServiceClassIDList（SDP 属性 0x0100）载荷：
    /// 元素 0x19 = UUID16、0x1A = UUID32、0x1C = UUID128；OPP UUID 为 0x1105。
    /// 返回 (是否包含 OPP, 是否识别到任意 UUID 元素)。
    /// </summary>
    internal static (bool FoundOpp, bool SawUuid) ScanServiceClassList(byte[] payload)
    {
        var uuid128 = new byte[]
        {
            0x00, 0x00, 0x11, 0x05, 0x00, 0x00, 0x10, 0x00,
            0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB
        };
        var foundOpp = false;
        var sawUuid = false;
        for (var i = 0; i < payload.Length; i++)
        {
            switch (payload[i])
            {
                case 0x19 when i + 2 < payload.Length:
                    sawUuid = true;
                    if (payload[i + 1] == 0x11 && payload[i + 2] == 0x05)
                        foundOpp = true;
                    break;
                case 0x1A when i + 4 < payload.Length:
                    sawUuid = true;
                    if (payload[i + 1] == 0x00 && payload[i + 2] == 0x00 &&
                        payload[i + 3] == 0x11 && payload[i + 4] == 0x05)
                        foundOpp = true;
                    break;
                case 0x1C when i + 16 < payload.Length:
                    sawUuid = true;
                    if (payload.AsSpan(i + 1, 16).SequenceEqual(uuid128))
                        foundOpp = true;
                    break;
            }
        }
        return (foundOpp, sawUuid);
    }

    private async Task<bool> SendSourceAsync(
        string deviceAddr, string sourcePath, string displayName, string mimeType,
        string localPathForRecord, CancellationToken ct)
    {
        OppDeviceInfo device;
        try
        {
            device = await ResolveDeviceAsync(deviceAddr, ct);
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", ex.Message));
            WriteFailedRecord("", deviceAddr, displayName, 0, ex.Message);
            return false;
        }

        if (!device.IsPaired)
        {
            var message = $"设备未配对：{device.Name}（{device.AddrDisplay}），请先在应用内或系统设置中完成配对";
            _events.Publish(new LogEvent("ERROR", message));
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, "设备未配对");
            return false;
        }

        var taskId = (uint)Random.Shared.Next();
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

        var retryPolicy = new OppRetryPolicy(_config.OppRetryCount, _config.OppRetryDelaySeconds);
        var attempt = 1;
        string? lastError = null;
        while (true)
        {
            try
            {
                await PushOnceAsync(device, sourcePath, displayName, mimeType, totalLength, checksum, localPathForRecord, taskId, ct);
                return true;
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
                WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消");
                return false;
            }
            catch (OperationCanceledException)
            {
                lastError = "连接或发送超时";
            }
            catch (Exception ex)
            {
                lastError = ex.Message;
            }

            if (retryPolicy.ShouldRetry(attempt, lastError))
            {
                var delay = retryPolicy.NextDelay(attempt);
                _events.Publish(new LogEvent("WARN",
                    $"OPP 推送失败（{displayName}）：{lastError}；{delay.TotalSeconds:0} 秒后第 {attempt + 1}/{retryPolicy.MaxAttempts} 次重试"));
                attempt++;
                try
                {
                    await Task.Delay(delay, ct);
                }
                catch (OperationCanceledException)
                {
                    _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
                    WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消");
                    return false;
                }
                continue;
            }

            var note = attempt > 1 ? $"{lastError}（已重试 {attempt - 1} 次）" : lastError;
            _events.Publish(new LogEvent("ERROR", $"OPP 推送失败：{displayName}（{note}）"));
            WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, note ?? "未知错误");
            return false;
        }
    }

    /// <summary>单次推送尝试：连接 → CONNECT → 流式 PUT → DISCONNECT，成功后写成功记录。</summary>
    private async Task PushOnceAsync(
        OppDeviceInfo device, string sourcePath, string displayName, string mimeType,
        long totalLength, string checksum, string localPathForRecord, uint taskId, CancellationToken ct)
    {
        using var connectCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        connectCts.CancelAfter(TimeSpan.FromSeconds(Math.Max(1, _config.OppConnectTimeoutSeconds)));
        await using var transport = await OpenTransportAsync(device, connectCts.Token);
        await using var client = new ObexClient(transport, new ObexOptions { NameUseBom = _config.OppNameUseBom }, _events, _config.OppAuthPassword);
        await client.ConnectAsync(connectCts.Token);

        using var sendCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        sendCts.CancelAfter(TimeSpan.FromSeconds(Math.Max(1, _config.OppSendTimeoutSeconds)));
        using var fs = new FileStream(sourcePath, FileMode.Open, FileAccess.Read, FileShare.Read);
        var result = await client.PushAsync(displayName, mimeType, totalLength,
            async (offset, buffer, count, token) =>
            {
                fs.Seek(offset, SeekOrigin.Begin);
                var n = await fs.ReadAsync(buffer.AsMemory(0, count), token);
                _events.Publish(new TransferProgressEvent(taskId.ToString(), offset + n, totalLength, 0));
                return n;
            },
            _config.OppChunkSize, sendCts.Token);

        _events.Publish(new TransferProgressEvent(taskId.ToString(), totalLength, totalLength, 0));
        try
        {
            await client.DisconnectAsync(sendCts.Token);
        }
        catch (Exception ex)
        {
            // 部分接收端（如 Android）在收到最终 PUT 后即主动断开，
            // DISCONNECT 失败不代表推送失败，仅记录警告。
            _events.Publish(new LogEvent("WARN", $"推送成功但 DISCONNECT 异常（可忽略）：{ex.Message}"));
        }

        _storage.UpsertDevice(new DeviceInfo
        {
            Addr = device.Addr,
            Name = device.Name,
            LastSeen = DateTime.Now.ToString("o"),
            LastConnected = DateTime.Now.ToString("o")
        });
        _events.Publish(new LogEvent("INFO",
            $"OPP 推送成功：{displayName} -> {device.Name}（{device.AddrDisplay}，{result.BytesSent} 字节）"));
        WriteOkRecord(device, displayName, totalLength, checksum, localPathForRecord, result.BytesSent);
    }

    private void WriteOkRecord(OppDeviceInfo device, string displayName, long size, string checksum, string localPath, long bytesSent)
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
            Channel = TransferConst.ChannelOpp,
            LocalPath = localPath,
            Note = $"已发送 {bytesSent} 字节"
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
            Channel = TransferConst.ChannelOpp,
            Note = note
        });
    }

    private static string MimeForName(string fileName)
    {
        var ext = Path.GetExtension(fileName).ToLowerInvariant();
        return ext switch
        {
            ".txt" => "text/plain",
            ".zip" => "application/zip",
            ".pdf" => "application/pdf",
            ".jpg" or ".jpeg" => "image/jpeg",
            ".png" => "image/png",
            ".mp3" => "audio/mpeg",
            ".mp4" => "video/mp4",
            ".doc" or ".docx" => "application/msword",
            ".xls" or ".xlsx" => "application/vnd.ms-excel",
            _ => "application/octet-stream"
        };
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
