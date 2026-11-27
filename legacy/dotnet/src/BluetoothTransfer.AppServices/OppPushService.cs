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
public class OppPushService : IOppPushService
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;
    private readonly DeviceInfoCache _deviceCache;

    public OppPushService(EventBus events, StorageService storage, AppConfig config,
        OppDiscoveryService? discovery = null, DeviceInfoCache? deviceCache = null)
    {
        _events = events ?? throw new ArgumentNullException(nameof(events));
        _storage = storage ?? throw new ArgumentNullException(nameof(storage));
        _config = config ?? throw new ArgumentNullException(nameof(config));
        _discovery = discovery ?? new OppDiscoveryService(events);
        // 默认给每个服务实例独立缓存（TTL 60s）：同一发送队列连续推多个文件时只解析一次设备。
        _deviceCache = deviceCache ?? new DeviceInfoCache();
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
            // 记录保持 Type=file + 原文件路径（临时 zip 会被清理，原文件才是可复制目标）。
            return await PushSendHelper.SendFileZipAsync(_events, filePath, "bt_opp",
                (src, dn, lp, token) => SendSourceAsync(deviceAddr, src, dn, "application/zip", lp, token,
                    TransferConst.TypeFile), ct);
        }
        return await SendSourceAsync(deviceAddr, filePath, info.Name, MimeForName(info.Name), filePath, ct,
            TransferConst.TypeFile);
    }

    public async Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default)
    {
        // 文本记录：Type=text + 正文入 content 列，复制时无需再依赖发送后即被清理的临时 .txt。
        var (content, contentNote) = PushSendHelper.PrepareTextRecordContent(text);
        // 写临时 .txt → 发送 → 清理临时文件的流程与助手通道共用，见 PushSendHelper。
        return await PushSendHelper.SendTextAsync(_events, _config, text, name, "bt_opp",
            (src, dn, lp, token) => SendSourceAsync(deviceAddr, src, dn, "text/plain", lp, token,
                TransferConst.TypeText, content, contentNote), ct);
    }

    public async Task<bool> SendFolderAsync(string deviceAddr, string folderPath, CancellationToken ct = default)
    {
        // 打包 → 发送 → 清理临时 zip 的流程与助手通道共用，见 PushSendHelper。
        // 记录 Type=folder + 原始文件夹路径（PushSendHelper 会把 folderPath 作为记录本地路径传入；
        // 临时 zip 发送后即删除，原始文件夹才是复制目标）。
        return await PushSendHelper.SendFolderZipAsync(_events, folderPath, "bt_opp",
            (src, dn, lp, token) => SendSourceAsync(deviceAddr, src, dn, "application/zip", lp, token,
                TransferConst.TypeFolder), ct);
    }

    /// <summary>按地址解析设备（测试可通过子类覆写注入假设备）。</summary>
    protected virtual async Task<OppDeviceInfo> ResolveDeviceAsync(string deviceAddr, CancellationToken ct)
    {
        var device = await _discovery.FindByAddressAsync(deviceAddr, ct);
        if (device == null)
            throw new ObexException($"未找到支持 OPP 的蓝牙设备：{deviceAddr}（请先扫描并确认设备已配对）");
        return device;
    }

    /// <summary>
    /// 带缓存的设备解析：TTL 内直接复用上次解析结果，避免每个文件都做一次全量 OPP 发现/SDP 枚举。
    /// 解析失败不缓存；连接/发送失败由调用方 <see cref="DeviceInfoCache.Invalidate"/> 后重试时重新解析。
    /// </summary>
    private async Task<OppDeviceInfo> ResolveDeviceWithCacheAsync(string deviceAddr, CancellationToken ct)
    {
        if (_deviceCache.TryGet(deviceAddr, out var cached))
        {
            _events.Publish(new LogEvent("DEBUG", $"命中设备解析缓存：{cached.Name}（{cached.Addr}）"));
            return new OppDeviceInfo
            {
                Id = cached.Id,
                Addr = cached.Addr,
                Name = cached.Name,
                IsPaired = cached.IsPaired
            };
        }

        var device = await ResolveDeviceAsync(deviceAddr, ct);
        _deviceCache.Set(device.Addr, device.Id, device.Name, device.IsPaired);
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
        string localPathForRecord, CancellationToken ct = default,
        string recordType = TransferConst.TypeFile, string recordContent = "", string recordNote = "")
    {
        // 并行启动 SHA-256（读盘/CPU）与设备解析（OPP 发现/SDP 枚举）：两者互不依赖，
        // 大文件与多文件连续发送时能明显减少“首字节”前的串行等待。取消经 WaitAsync 立即传播；
        // FileChecksum 无取消重载，被放弃的哈希会在后台自然读完（异常由 ObserveFault 观察）。
        var hashTask = FileChecksum.ComputeSha256Async(sourcePath);

        OppDeviceInfo device;
        try
        {
            device = await ResolveDeviceWithCacheAsync(deviceAddr, ct);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            PushSendHelper.ObserveFault(hashTask);
            _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
            WriteFailedRecord("", deviceAddr, displayName, 0, "用户取消", recordType);
            return false;
        }
        catch (Exception ex)
        {
            PushSendHelper.ObserveFault(hashTask);
            _events.Publish(new LogEvent("ERROR", ex.Message));
            WriteFailedRecord("", deviceAddr, displayName, 0, ex.Message, recordType);
            return false;
        }

        if (!device.IsPaired)
        {
            // 配对状态可能刚变化（用户刚配对/解除配对）：不缓存该结论，下次发送重新解析。
            _deviceCache.Invalidate(deviceAddr);
            PushSendHelper.ObserveFault(hashTask);
            var message = $"设备未配对：{device.Name}（{device.AddrDisplay}），请先在应用内或系统设置中完成配对";
            _events.Publish(new LogEvent("ERROR", message));
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, "设备未配对", recordType);
            return false;
        }

        long totalLength;
        string checksum;
        try
        {
            totalLength = new FileInfo(sourcePath).Length;
            checksum = await hashTask.WaitAsync(ct);
        }
        catch (OperationCanceledException) when (ct.IsCancellationRequested)
        {
            PushSendHelper.ObserveFault(hashTask);
            _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, "用户取消", recordType);
            return false;
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"计算校验和失败：{ex.Message}"));
            WriteFailedRecord(device.Name, device.Addr, displayName, 0, ex.Message, recordType);
            return false;
        }

        var retryPolicy = new OppRetryPolicy(_config.OppRetryCount, _config.OppRetryDelaySeconds);
        var taskId = (uint)Random.Shared.Next();
        var attempt = 1;
        string? lastError = null;
        while (true)
        {
            try
            {
                await PushOnceAsync(device, sourcePath, displayName, mimeType, totalLength, checksum,
                    localPathForRecord, taskId, ct, recordType, recordContent, recordNote);
                return true;
            }
            catch (OperationCanceledException) when (ct.IsCancellationRequested)
            {
                _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
                WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消", recordType);
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

            // 连接/发送失败可能意味着缓存的 WinRT 设备 Id/服务条目已失效（重启蓝牙、重新配对会更换 Id）：
            // 先失效缓存，若允许重试则在退避后重新解析一次；重新解析失败时沿用上次结果，
            // 避免把“可重试的传输错误”直接变成“解析失败”。
            _deviceCache.Invalidate(deviceAddr);

            if (retryPolicy.ShouldRetry(attempt, lastError))
            {
                var delay = retryPolicy.NextDelay(attempt);
                _events.Publish(new LogEvent("WARN",
                    $"OPP 推送失败（{displayName}）：{lastError}；{delay.TotalSeconds:0} 秒后第 {attempt + 1}/{retryPolicy.MaxAttempts} 次重试"));
                attempt++;
                if (!await PushSendHelper.TryDelayAsync(delay, ct))
                {
                    _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
                    WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消", recordType);
                    return false;
                }
                try
                {
                    device = await ResolveDeviceWithCacheAsync(deviceAddr, ct);
                }
                catch (OperationCanceledException) when (ct.IsCancellationRequested)
                {
                    _events.Publish(new LogEvent("WARN", $"OPP 推送已取消：{displayName}"));
                    WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, "用户取消", recordType);
                    return false;
                }
                catch (Exception ex)
                {
                    _events.Publish(new LogEvent("WARN", $"重试前重新解析设备失败，沿用上次解析结果：{ex.Message}"));
                }
                continue;
            }

            var note = attempt > 1 ? $"{lastError}（已重试 {attempt - 1} 次）" : lastError;
            _events.Publish(new LogEvent("ERROR", $"OPP 推送失败：{displayName}（{note}）"));
            WriteFailedRecord(device.Name, device.Addr, displayName, totalLength, note ?? "未知错误", recordType);
            return false;
        }
    }

    /// <summary>单次推送尝试：连接 → CONNECT → 流式 PUT → DISCONNECT，成功后写成功记录。</summary>
    private async Task PushOnceAsync(
        OppDeviceInfo device, string sourcePath, string displayName, string mimeType,
        long totalLength, string checksum, string localPathForRecord, uint taskId, CancellationToken ct,
        string recordType = TransferConst.TypeFile, string recordContent = "", string recordNote = "")
    {
        // 连接阶段：仍使用一次性连接超时（OppConnectTimeoutSeconds），与发送阶段相互独立。
        using var connectCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        connectCts.CancelAfter(TimeSpan.FromSeconds(Math.Max(1, _config.OppConnectTimeoutSeconds)));
        await using var transport = await OpenTransportAsync(device, connectCts.Token);
        await using var client = new ObexClient(transport, new ObexOptions { NameUseBom = _config.OppNameUseBom }, _events, _config.OppAuthPassword);
        await client.ConnectAsync(connectCts.Token);
        // 连接阶段完成：停掉连接计时器，后续发送阶段只受下面的“空闲超时”约束，避免长传输期间
        // 连接超时定时器无谓触发（当前已无调用点使用该令牌，此处显式停表表明阶段边界）。
        connectCts.CancelAfter(Timeout.InfiniteTimeSpan);

        // 发送阶段：OppSendTimeoutSeconds 的语义是「空闲超时」，不是「整次发送总超时」。
        // 不再对整段 PushAsync 一次性 CancelAfter；改为先启动计时，然后 PushAsync 在读取/发送
        // 每一个分片前都会回调下面的 readChunk，在回调里重置计时（CancelAfter）。
        // 效果：只要对端持续响应、传输持续有进展，>100 MB 的大文件不会在 300 秒处被整体判死；
        // 而连接静默/卡住（长时间读不到响应、无任何分片进展）仍会在空闲超时后失败。
        var idleTimeout = TimeSpan.FromSeconds(Math.Max(1, _config.OppSendTimeoutSeconds));
        using var sendCts = CancellationTokenSource.CreateLinkedTokenSource(ct);
        sendCts.CancelAfter(idleTimeout);
        using var fs = new FileStream(sourcePath, FileMode.Open, FileAccess.Read, FileShare.Read);
        var result = await client.PushAsync(displayName, mimeType, totalLength,
            async (offset, buffer, count, token) =>
            {
                // 每分片读取前重置空闲计时（首包/头包阶段由上面的初始 CancelAfter 覆盖）。
                sendCts.CancelAfter(idleTimeout);
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
        WriteOkRecord(device, displayName, totalLength, checksum, localPathForRecord, result.BytesSent,
            recordType, recordContent, recordNote);
    }

    private void WriteOkRecord(OppDeviceInfo device, string displayName, long size, string checksum,
        string localPath, long bytesSent, string recordType = TransferConst.TypeFile,
        string recordContent = "", string recordNote = "")
    {
        PushSendHelper.WriteOkRecord(_storage, TransferConst.ChannelOpp,
            device.Name, device.Addr, displayName, size, checksum, localPath, bytesSent,
            type: recordType, content: recordContent, extraNote: recordNote);
    }

    private void WriteFailedRecord(string peerName, string peerAddr, string displayName, long size,
        string note, string recordType = TransferConst.TypeFile)
    {
        PushSendHelper.WriteFailedRecord(_storage, TransferConst.ChannelOpp, peerName, peerAddr, displayName, size, note, recordType);
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
    /// type/content 为第二阶段新增（默认 Type=file + 无正文，保持单文件/--zip 记录格式不变）：
    /// 文本记录传 Type=text + 正文；文件夹记录传 Type=folder；正文超限时由调用方传空 content + extraNote 说明。
    /// </summary>
    internal static void WriteOkRecord(
        StorageService storage, string channel, string peerName, string peerAddr,
        string displayName, long size, string checksum, string localPath, long bytesSent,
        string resumeNote = "", string type = TransferConst.TypeFile, string content = "",
        string extraNote = "")
    {
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = string.IsNullOrEmpty(type) ? TransferConst.TypeFile : type,
            PeerName = peerName,
            PeerAddr = peerAddr,
            Name = displayName,
            Size = size,
            Status = TransferConst.StatusOk,
            Checksum = checksum,
            Channel = channel,
            LocalPath = localPath,
            Content = content ?? "",
            Note = $"已发送 {bytesSent} 字节{resumeNote}{extraNote}"
        });
    }

    /// <summary>失败记录写入模板：通道作为参数传入；type 用于让文本/文件夹的失败记录保持正确类型显示。</summary>
    internal static void WriteFailedRecord(
        StorageService storage, string channel, string peerName, string peerAddr,
        string displayName, long size, string note, string type = TransferConst.TypeFile)
    {
        storage.AddRecord(new TransferRecord
        {
            Direction = TransferConst.DirSend,
            Type = string.IsNullOrEmpty(type) ? TransferConst.TypeFile : type,
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
    /// 准备文本记录的正文与附注：正文按 UTF-8 字节数校验
    /// <see cref="TransferConst.MaxRecordContentLength"/> 上限，超限时正文留空并返回中文附注
    /// （写入记录 Note，复制该记录时由规划器给出中文提示），避免超大文本撑大 SQLite 记录。
    /// </summary>
    internal static (string Content, string Note) PrepareTextRecordContent(string? text)
    {
        var value = text ?? "";
        var byteCount = Encoding.UTF8.GetByteCount(value);
        if (byteCount <= TransferConst.MaxRecordContentLength) return (value, "");
        return ("", $"（正文 {byteCount} 字节，超过 {TransferConst.MaxRecordContentLength} 字节的记录内容上限，未保存正文）");
    }

    /// <summary>观察（并吞掉）一个可能被提前放弃的 Task 的异常，避免未观察异常：仅用于并行哈希等旁路任务。</summary>
    internal static void ObserveFault(Task task)
    {
        _ = task.ContinueWith(static t => _ = t.Exception, CancellationToken.None,
            TaskContinuationOptions.OnlyOnFaulted | TaskContinuationOptions.ExecuteSynchronously,
            TaskScheduler.Default);
    }

    /// <summary>
    /// 等旁路任务（如并行 SHA-256）结束后再执行清理，并观察其异常。
    /// 用于「哈希仍在读打包产物时提前返回失败」的路径：立即删除会因 Windows 文件共享限制失败，
    /// 产物要滞留到孤儿清理（7 天）才被回收。
    /// </summary>
    internal static void CleanupAfterTask(Task task, Action cleanup)
    {
        _ = task.ContinueWith(t =>
        {
            _ = t.Exception; // 观察异常，避免未观察任务异常
            try { cleanup(); }
            catch { /* 清理失败不改变发送结果，由孤儿清理兜底 */ }
        }, CancellationToken.None, TaskContinuationOptions.ExecuteSynchronously, TaskScheduler.Default);
    }

    /// <summary>可取消的退避等待：被取消返回 false（两条推送通道共用，保持取消语义一致）。</summary>
    internal static async Task<bool> TryDelayAsync(TimeSpan delay, CancellationToken ct)
    {
        try
        {
            await Task.Delay(delay, ct);
            return true;
        }
        catch (OperationCanceledException)
        {
            return false;
        }
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
    /// 注意 sendAsync 的第三个参数（记录本地路径）传<b>原始文件夹路径</b>而不是临时 zip：
    /// 临时 zip 发送后即被清理，只有原始文件夹能在发送后继续用于「复制到剪贴板」。
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
            return await sendAsync(zipPath, folderName + ".zip", folderPath, ct);
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
