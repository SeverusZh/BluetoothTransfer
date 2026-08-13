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
            // 打包 → 发送 → 清理临时 zip 的流程与助手通道共用，见 PushSendHelper。
            return await PushSendHelper.SendFileZipAsync(_events, filePath, "bt_opp",
                (src, dn, lp, token) => SendSourceAsync(deviceAddr, src, dn, "application/zip", lp, token), ct);
        }
        return await SendSourceAsync(deviceAddr, filePath, info.Name, MimeForName(info.Name), filePath, ct);
    }

    public async Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default)
    {
        // 写临时 .txt → 发送 → 清理临时文件的流程与助手通道共用，见 PushSendHelper。
        return await PushSendHelper.SendTextAsync(_events, _config, text, name, "bt_opp",
            (src, dn, lp, token) => SendSourceAsync(deviceAddr, src, dn, "text/plain", lp, token), ct);
    }

    public async Task<bool> SendFolderAsync(string deviceAddr, string folderPath, CancellationToken ct = default)
    {
        // 打包 → 发送 → 清理临时 zip 的流程与助手通道共用，见 PushSendHelper。
        return await PushSendHelper.SendFolderZipAsync(_events, folderPath, "bt_opp",
            (src, dn, lp, token) => SendSourceAsync(deviceAddr, src, dn, "application/zip", lp, token), ct);
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
            _events.Publish(new LogEvent("WARN", $"OPP SDP 预检未完成（继续尝试连接）：{FormatError(ex)}"));
        }
    }

    /// <summary>异常消息为空时回退到类型名 + HRESULT，便于定位 WinRT 错误。</summary>
    internal static string FormatError(Exception ex)
        => string.IsNullOrWhiteSpace(ex.Message)
            ? $"{ex.GetType().Name} (0x{ex.HResult:X8})"
            : ex.Message;

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
                lastError = FormatError(ex);
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
        PushSendHelper.WriteOkRecord(_storage, TransferConst.ChannelOpp,
            device.Name, device.Addr, displayName, size, checksum, localPath, bytesSent);
    }

    private void WriteFailedRecord(string peerName, string peerAddr, string displayName, long size, string note)
    {
        PushSendHelper.WriteFailedRecord(_storage, TransferConst.ChannelOpp, peerName, peerAddr, displayName, size, note);
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
}

/// <summary>
/// 推送共用骨架（静态辅助组合）：把 OPP 与助手两条通道高度重复的
/// 「打包（zip）/写临时 .txt → 发送 → 计算校验和 → 写记录 → 清理临时文件」流程
/// 收敛到此，避免改一处漏另一处。
/// 真正的传输发送由各服务通过 sendAsync 委托注入（闭包携带各自的通道参数、mimeType 与记录本地路径）。
/// 重试循环因两条通道的传输/错误分类差异较大（OPP 走异常驱动 + 超时，助手处理“忙”与续传偏移），
/// 故保留在各服务自身的 SendSourceAsync 中，此处只共享与传输无关的骨架。
/// </summary>
internal static class PushSendHelper
{
    /// <summary>尝试删除临时文件，失败不阻塞流程。OPP 与助手通道共用。</summary>
    internal static void TryDelete(string path)
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

    /// <summary>
    /// 成功记录写入模板：通道（Channel 字段值）作为参数传入，OOP/助手各自映射到自己的常量。
    /// resumeNote 仅助手通道用于标记续传偏移，缺省为空以保持 OPP 记录格式不变。
    /// </summary>
    internal static void WriteOkRecord(
        StorageService storage, string channel, string peerName, string peerAddr,
        string displayName, long size, string checksum, string localPath, long bytesSent, string resumeNote = "")
    {
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeFile,
            PeerName = peerName,
            PeerAddr = peerAddr,
            Name = displayName,
            Size = size,
            Status = TransferConst.StatusOk,
            Checksum = checksum,
            Channel = channel,
            LocalPath = localPath,
            Note = $"已发送 {bytesSent} 字节{resumeNote}"
        });
    }

    /// <summary>失败记录写入模板：通道作为参数传入。</summary>
    internal static void WriteFailedRecord(
        StorageService storage, string channel, string peerName, string peerAddr,
        string displayName, long size, string note)
    {
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = TransferConst.TypeFile,
            PeerName = peerName,
            PeerAddr = peerAddr,
            Name = displayName,
            Size = size,
            Status = TransferConst.StatusFailed,
            Channel = channel,
            Note = note
        });
    }

    /// <summary>
    /// 单文件 zip 推送骨架：把单个文件打入临时 .zip → sendAsync 发送 → 清理临时 zip。
    /// tempPrefix 为通道前缀（如 bt_opp/bt_asst），保持各通道临时文件命名与清理匹配不变。
    /// </summary>
    internal static async Task<bool> SendFileZipAsync(
        EventBus events, string filePath, string tempPrefix,
        Func<string, string, string, CancellationToken, Task<bool>> sendAsync,
        CancellationToken ct)
    {
        var entryName = new FileInfo(filePath).Name;
        var zipPath = Path.Combine(Path.GetTempPath(), $"{tempPrefix}_zip_{Guid.NewGuid():N}.zip");
        try
        {
            await Task.Run(() =>
            {
                using var archive = ZipFile.Open(zipPath, ZipArchiveMode.Create);
                archive.CreateEntryFromFile(filePath, entryName, CompressionLevel.Optimal);
            }, ct);
            return await sendAsync(zipPath, entryName + ".zip", filePath, ct);
        }
        catch (Exception ex)
        {
            events.Publish(new LogEvent("ERROR", $"文件打包失败：{ex.Message}"));
            return false;
        }
        finally
        {
            TryDelete(zipPath);
        }
    }

    /// <summary>
    /// 文件夹 zip 推送骨架：校验文件夹 → 打包为临时 .zip → sendAsync 发送 → 清理临时 zip。
    /// 打包错误文案与 OPP/助手两通道原先保持一致。
    /// </summary>
    internal static async Task<bool> SendFolderZipAsync(
        EventBus events, string folderPath, string tempPrefix,
        Func<string, string, string, CancellationToken, Task<bool>> sendAsync,
        CancellationToken ct)
    {
        if (string.IsNullOrWhiteSpace(folderPath) || !Directory.Exists(folderPath))
        {
            events.Publish(new LogEvent("ERROR", $"文件夹不存在：{folderPath}"));
            return false;
        }
        var folderName = Path.GetFileName(folderPath.TrimEnd('\\', '/'));
        if (string.IsNullOrEmpty(folderName)) folderName = "folder";
        var zipPath = Path.Combine(Path.GetTempPath(), $"{tempPrefix}_folder_{Guid.NewGuid():N}.zip");
        try
        {
            await Task.Run(() => ZipFile.CreateFromDirectory(folderPath, zipPath, CompressionLevel.Optimal, includeBaseDirectory: false), ct);
            return await sendAsync(zipPath, folderName + ".zip", zipPath, ct);
        }
        catch (Exception ex)
        {
            events.Publish(new LogEvent("ERROR", $"文件夹打包失败：{ex.Message}"));
            return false;
        }
        finally
        {
            TryDelete(zipPath);
        }
    }

    /// <summary>
    /// 文本推送骨架：写临时 .txt → sendAsync 发送 → 清理临时文件。
    /// 文件名缺省取配置 PushTextFileName；临时命名与清理匹配 OPP/助手两通道。
    /// </summary>
    internal static async Task<bool> SendTextAsync(
        EventBus events, AppConfig config, string text, string? name, string tempPrefix,
        Func<string, string, string, CancellationToken, Task<bool>> sendAsync,
        CancellationToken ct)
    {
        var fileName = string.IsNullOrWhiteSpace(name) ? config.PushTextFileName : name;
        var tempPath = Path.Combine(Path.GetTempPath(), $"{tempPrefix}_text_{Guid.NewGuid():N}.txt");
        try
        {
            await File.WriteAllBytesAsync(tempPath, Encoding.UTF8.GetBytes(text ?? ""), ct);
            return await sendAsync(tempPath, fileName, tempPath, ct);
        }
        catch (Exception ex)
        {
            events.Publish(new LogEvent("ERROR", $"文本临时文件写入失败：{ex.Message}"));
            return false;
        }
        finally
        {
            TryDelete(tempPath);
        }
    }
}
