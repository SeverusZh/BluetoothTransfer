using System.Diagnostics;
using System.Text.Encodings.Web;
using System.Text.Json;
using System.Text.Json.Serialization;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>收件箱 JSON 的载荷（仅本文件使用；字段名经 camelCase 策略序列化）。</summary>
internal sealed class ExternalSendInboxPayload
{
    /// <summary>显式无参构造：System.Text.Json 反射反序列化依赖它（internal 类型的隐式构造可见性不够明确）。</summary>
    public ExternalSendInboxPayload()
    {
    }

    public int Version { get; set; }

    public List<ExternalSendInboxItem> Items { get; set; } = new();

    /// <summary>解析阶段的提示（路径失效、文本回退等），由主实例展示给用户，保证跨进程不丢提示。</summary>
    public List<string>? Warnings { get; set; }
}

/// <summary>收件箱 JSON 中的单个发送项。</summary>
internal sealed class ExternalSendInboxItem
{
    public ExternalSendInboxItem()
    {
    }

    /// <summary>file / folder / text，与 <see cref="TransferConst"/> 的记录类型取值一致。</summary>
    public string Kind { get; set; } = "";

    public string Path { get; set; } = "";

    /// <summary>仅 text 项携带：--send-text 读出的正文。</summary>
    public string? Text { get; set; }
}

/// <summary>
/// 二次实例 → 主实例的发送载荷收件箱（纯文件逻辑，无 WPF/Windows 依赖，可跨平台单测）。
///
/// 背景：资源管理器右键菜单/SendTo 在应用已运行时只会再拉起一个进程；单实例守卫
/// （<see cref="SingleInstanceGuard"/>，本类不修改它）只提供「激活已有窗口」的命名事件信号，
/// 不携带命令行载荷。因此约定：
/// 1) 二次实例把 <see cref="ShellSendRequest"/> 序列化为
///    <c>%LOCALAPPDATA%\BluetoothTransfer\inbox\&lt;guid&gt;.json</c>，再发激活信号；
/// 2) 主实例在 UI 线程排空该目录（应用启动时也排空一次，兜住上次崩溃/信号丢失的残留）；
/// 3) 多选时资源管理器可能逐项拉起多个进程（每个进程一份 JSON），主实例用 ~400ms 去抖合并成一批。
///
/// 安全与健壮性：
/// - 原子写：先写 <c>&lt;guid&gt;.json.tmp</c> 再 <c>File.Move</c>，主实例永远看不到半截文件；
/// - 单文件上限 <see cref="MaxJsonBytes"/>（1MB），超限/解析失败/内容非法的文件删除并记录中文日志；
/// - 只接受已存在路径（写入前与排空时各校验一次），跨文件/跨项大小写不敏感去重，总数 ≤ <see cref="ShellSendRequest.MaxItems"/>；
/// - 只枚举/删除本目录内的文件（<see cref="IsOwned"/>），绝不触碰用户文件；
/// - 过期残留：<see cref="CleanResidue"/> 清理超过 <see cref="DefaultResidueMaxAge"/> 的 JSON 与中断写入的 .tmp。
/// </summary>
public sealed class ExternalSendInbox
{
    /// <summary>收件箱目录名（位于 %LOCALAPPDATA%\BluetoothTransfer 下，与 %APPDATA% 的 journal/记录库分离）。</summary>
    public const string DefaultDirectoryName = "inbox";

    /// <summary>单个收件箱 JSON 的大小上限（1MB）：正常请求只有少量短路径/短文本，超限视为异常并丢弃。</summary>
    public const int MaxJsonBytes = 1024 * 1024;

    /// <summary>JSON 载荷版本号：字段变化时递增；读到其它版本时仅告警并按当前字段尽力兼容读取。</summary>
    private const int PayloadVersion = 1;

    private const string JsonExtension = ".json";
    private const string TempExtension = ".tmp";

    /// <summary>过期残留保留时间：超过后启动时直接清理（崩溃残留兜底，避免旧右键请求在数天后突然弹窗）。</summary>
    public static readonly TimeSpan DefaultResidueMaxAge = TimeSpan.FromDays(7);

    /// <summary>临时文件保留时间：原子写的中转文件正常情况下毫秒级即消失，超过该时间说明写入进程已崩溃。</summary>
    private static readonly TimeSpan TempResidueMaxAge = TimeSpan.FromHours(1);

    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        // 中文不转义为 \uXXXX：既保持可读，也避免中文文本把 JSON 撑到上限（与 GUI 进程内使用一致）。
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
        WriteIndented = false
    };

    /// <summary>
    /// 创建收件箱。<paramref name="directory"/> 为空时使用 <see cref="DefaultDirectory"/>；
    /// 注入目录仅用于测试，生产路径固定为当前用户目录（不读配置、不接受命令行指定，避免被利用写任意目录）。
    /// </summary>
    public ExternalSendInbox(string? directory = null)
    {
        var resolved = string.IsNullOrWhiteSpace(directory) ? DefaultDirectory : directory;
        InboxDirectory = Path.GetFullPath(resolved);
    }

    /// <summary>默认收件箱目录：<c>%LOCALAPPDATA%\BluetoothTransfer\inbox</c>。</summary>
    public static string DefaultDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "BluetoothTransfer",
        DefaultDirectoryName);

    /// <summary>收件箱目录（绝对路径）。</summary>
    public string InboxDirectory { get; }

    /// <summary>
    /// 把发送请求原子写入收件箱，供主实例排空后入队。
    /// 允许「只有提示、没有可发送项」的请求（items 为空、warnings 非空）：用于把二次实例的解析失败原因
    /// （路径已失效等）转发给主实例展示，保证右键后不静默。
    /// 返回 false 时 <paramref name="error"/> 为可直接展示的中文原因（调用方负责提示用户，不静默）。
    /// </summary>
    public bool TryWrite(ShellSendRequest request, out string error)
    {
        error = "";
        if (request == null)
        {
            error = "待转发的发送请求为空";
            return false;
        }
        if (!request.HasItems && request.Warnings.Count == 0)
        {
            error = "待转发的发送请求中没有可发送项";
            return false;
        }

        try
        {
            var items = new List<ExternalSendInboxItem>();
            var warnings = new List<string>(request.Warnings);
            var missing = new List<string>();

            foreach (var item in request.Items)
            {
                // 二次实例与主实例之间文件可能被移动/删除：写入前再校验一次，只转发仍然存在的路径。
                var exists = item.Kind == ShellSendItemKind.Folder ? Directory.Exists(item.Path) : File.Exists(item.Path);
                if (!exists)
                {
                    missing.Add(item.Path);
                    continue;
                }
                items.Add(new ExternalSendInboxItem
                {
                    Kind = KindToWire(item.Kind),
                    Path = item.Path,
                    Text = item.Kind == ShellSendItemKind.Text ? item.Text : null
                });
            }

            if (missing.Count > 0)
            {
                var sample = string.Join("、", missing.Take(3));
                warnings.Add($"有 {missing.Count} 个待发送路径已不存在，未转发：{sample}{(missing.Count > 3 ? " 等" : "")}");
            }
            if (items.Count == 0 && warnings.Count == 0)
            {
                error = "待发送的文件/文件夹已不存在，未能转发给已运行的实例";
                return false;
            }

            var payload = new ExternalSendInboxPayload
            {
                Version = PayloadVersion,
                Items = items,
                Warnings = warnings.Count > 0 ? warnings : null
            };
            var bytes = JsonSerializer.SerializeToUtf8Bytes(payload, JsonOptions);
            if (bytes.Length > MaxJsonBytes)
            {
                error = $"待转发数据过大（{bytes.Length:N0} 字节，上限 {MaxJsonBytes:N0} 字节）";
                return false;
            }

            Directory.CreateDirectory(InboxDirectory);
            var finalPath = Path.Combine(InboxDirectory, Guid.NewGuid().ToString("N") + JsonExtension);
            var tempPath = finalPath + TempExtension;
            try
            {
                // 原子写：临时文件落盘后再改名，主实例枚举到的每个 *.json 都是完整载荷。
                File.WriteAllBytes(tempPath, bytes);
                File.Move(tempPath, finalPath, overwrite: true);
            }
            catch
            {
                // 写入/改名失败时尽量清掉半成品；清理失败只影响目录观感，下一次启动的 CleanResidue 会兜底。
                TryDelete(tempPath, log: null);
                throw;
            }
            return true;
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or NotSupportedException
                                       or System.Security.SecurityException or JsonException or ArgumentException)
        {
            error = $"写入发送请求收件箱失败：{ex.Message}";
            return false;
        }
    }

    /// <summary>
    /// 排空收件箱：读出全部 <c>*.json</c>、合并去重、删除已读文件，返回汇总请求（无内容时返回
    /// <see cref="ShellSendRequest.Empty"/>）。文件 IO 在后台线程执行，避免阻塞 UI。
    /// </summary>
    /// <param name="log">中文日志回调（解析失败/超限/路径失效等），可为 null；调用方负责显示。</param>
    public Task<ShellSendRequest> DrainAsync(Action<string>? log = null, CancellationToken ct = default)
        => Task.Run(() => Drain(log, ct), ct);

    /// <summary>
    /// 清理过期残留（启动时调用）：
    /// 1) 超过 <paramref name="olderThan"/>（默认 7 天）的 JSON——正常路径下会在排空时消费，出现即说明应用长期未运行；
    /// 2) 超过 1 小时的 .tmp——原子写中断的残留。
    /// 返回删除的文件数。
    /// </summary>
    public int CleanResidue(TimeSpan? olderThan = null, Action<string>? log = null)
    {
        if (!Directory.Exists(InboxDirectory)) return 0;

        var maxAge = olderThan.HasValue && olderThan.Value > TimeSpan.Zero
            ? olderThan.Value
            : DefaultResidueMaxAge;
        var jsonCutoff = DateTime.UtcNow - maxAge;
        var tempCutoff = DateTime.UtcNow - TempResidueMaxAge;

        List<string> files;
        try
        {
            files = Directory.EnumerateFiles(InboxDirectory, "*", SearchOption.TopDirectoryOnly).ToList();
        }
        catch (Exception ex) when (IsIoException(ex))
        {
            Warn(log, $"读取发送请求收件箱失败：{ex.Message}");
            return 0;
        }

        var removed = 0;
        foreach (var file in files)
        {
            try
            {
                var extension = Path.GetExtension(file);
                if (string.Equals(extension, TempExtension, StringComparison.OrdinalIgnoreCase))
                {
                    if (SafeGetLastWriteTimeUtc(file) >= tempCutoff) continue;
                    if (TryDelete(file, log)) removed++;
                    continue;
                }
                if (!string.Equals(extension, JsonExtension, StringComparison.OrdinalIgnoreCase)) continue;
                if (SafeGetLastWriteTimeUtc(file) >= jsonCutoff) continue;
                if (TryDelete(file, log))
                {
                    removed++;
                    Warn(log, $"已清理过期的发送请求残留：{Path.GetFileName(file)}");
                }
            }
            catch (Exception ex) when (IsIoException(ex))
            {
                Warn(log, $"清理收件箱残留失败：{Path.GetFileName(file)}（{ex.Message}）");
            }
        }
        return removed;
    }

    /// <summary>同步排空实现（DrainAsync 在后台线程调用它；内部每个文件的异常都不会逃逸）。</summary>
    private ShellSendRequest Drain(Action<string>? log, CancellationToken ct)
    {
        var items = new List<ShellSendItem>();
        var warnings = new List<string>();
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var limitWarned = false;

        if (!Directory.Exists(InboxDirectory)) return ShellSendRequest.Empty;

        List<string> files;
        try
        {
            files = Directory.EnumerateFiles(InboxDirectory, "*" + JsonExtension, SearchOption.TopDirectoryOnly).ToList();
        }
        catch (Exception ex) when (IsIoException(ex))
        {
            Warn(log, $"读取发送请求收件箱失败：{ex.Message}");
            return ShellSendRequest.Empty;
        }

        // 文件名是随机 GUID，用写入时间近似还原用户选择/进程启动顺序；时间相同再按名称稳定排序。
        files.Sort((left, right) =>
        {
            var compare = SafeGetLastWriteTimeUtc(left).CompareTo(SafeGetLastWriteTimeUtc(right));
            return compare != 0 ? compare : string.CompareOrdinal(left, right);
        });

        foreach (var file in files)
        {
            if (ct.IsCancellationRequested) break;
            try
            {
                var info = new FileInfo(file);
                if (info.Length > MaxJsonBytes)
                {
                    Warn(log, $"收件箱请求文件超过 {MaxJsonBytes / (1024 * 1024)}MB 上限，已丢弃：{Path.GetFileName(file)}");
                    TryDelete(file, log);
                    continue;
                }

                byte[] bytes;
                try
                {
                    bytes = File.ReadAllBytes(file);
                }
                catch (Exception ex) when (IsIoException(ex))
                {
                    Warn(log, $"读取收件箱请求文件失败，已丢弃：{Path.GetFileName(file)}（{ex.Message}）");
                    TryDelete(file, log);
                    continue;
                }

                ExternalSendInboxPayload? payload = null;
                try
                {
                    payload = JsonSerializer.Deserialize<ExternalSendInboxPayload>(bytes, JsonOptions);
                }
                catch (Exception ex) when (ex is JsonException or NotSupportedException)
                {
                    Warn(log, $"收件箱请求文件解析失败，已丢弃：{Path.GetFileName(file)}（{ex.Message}）");
                    TryDelete(file, log);
                    continue;
                }
                if (payload == null || payload.Items == null)
                {
                    Warn(log, $"收件箱请求文件内容为空，已丢弃：{Path.GetFileName(file)}");
                    TryDelete(file, log);
                    continue;
                }
                if (payload.Version != PayloadVersion)
                    Warn(log, $"收件箱请求文件版本不匹配（{payload.Version}），按兼容方式读取：{Path.GetFileName(file)}");

                if (payload.Warnings != null)
                    warnings.AddRange(payload.Warnings.Where(w => !string.IsNullOrWhiteSpace(w)));

                foreach (var wire in payload.Items)
                {
                    if (items.Count >= ShellSendRequest.MaxItems)
                    {
                        if (!limitWarned)
                        {
                            limitWarned = true;
                            warnings.Add($"一次最多处理 {ShellSendRequest.MaxItems} 项，其余项已忽略");
                        }
                        break;
                    }
                    if (wire == null) continue;

                    var kind = KindFromWire(wire.Kind);
                    if (kind == null)
                    {
                        Warn(log, $"收件箱请求项类型无法识别，已跳过：{wire.Kind}");
                        continue;
                    }
                    if (!TryNormalizeInboxPath(wire.Path, out var fullPath))
                    {
                        Warn(log, $"收件箱请求项路径无效，已跳过：{wire.Path}");
                        continue;
                    }
                    if (!seen.Add(fullPath)) continue; // 跨文件/跨项静默去重，避免重复入队

                    var exists = kind == ShellSendItemKind.Folder ? Directory.Exists(fullPath) : File.Exists(fullPath);
                    if (!exists)
                    {
                        Warn(log, $"待发送路径已不存在，已跳过：{fullPath}");
                        continue;
                    }
                    items.Add(new ShellSendItem
                    {
                        Kind = kind.Value,
                        Path = fullPath,
                        Text = kind == ShellSendItemKind.Text ? wire.Text ?? "" : ""
                    });
                }

                // 读出后即删除：即使其中个别项无效，也避免下次启动重复提示同一条残留。
                TryDelete(file, log);
            }
            catch (Exception ex) when (IsIoException(ex))
            {
                Warn(log, $"处理收件箱请求文件失败，已丢弃：{Path.GetFileName(file)}（{ex.Message}）");
                TryDelete(file, log);
            }
        }

        return ShellSendRequest.FromItems(items, warnings);
    }

    /// <summary>删除收件箱目录内的文件；非本目录（或无法确认）一律拒绝，返回是否真的删除。</summary>
    private bool TryDelete(string path, Action<string>? log)
    {
        if (!IsOwned(path)) return false;
        try
        {
            if (!File.Exists(path)) return false;
            File.Delete(path);
            return true;
        }
        catch (Exception ex) when (IsIoException(ex))
        {
            Warn(log, $"删除收件箱文件失败：{Path.GetFileName(path)}（{ex.Message}）");
            return false;
        }
    }

    /// <summary>路径是否位于收件箱目录内（只删自己创建的 JSON/临时文件，绝不误删用户文件）。</summary>
    private bool IsOwned(string? path)
    {
        if (string.IsNullOrWhiteSpace(path)) return false;
        try
        {
            var full = Path.GetFullPath(path);
            var root = Path.TrimEndingDirectorySeparator(Path.GetFullPath(InboxDirectory));
            return full.StartsWith(root + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase);
        }
        catch (Exception ex) when (IsIoException(ex))
        {
            return false;
        }
    }

    private static string KindToWire(ShellSendItemKind kind) => kind switch
    {
        ShellSendItemKind.Folder => TransferConst.TypeFolder,
        ShellSendItemKind.Text => TransferConst.TypeText,
        _ => TransferConst.TypeFile
    };

    private static ShellSendItemKind? KindFromWire(string? kind) => kind switch
    {
        TransferConst.TypeFolder => ShellSendItemKind.Folder,
        TransferConst.TypeText => ShellSendItemKind.Text,
        TransferConst.TypeFile => ShellSendItemKind.File,
        _ => null
    };

    /// <summary>收件箱路径规范化：去空白/多余引号 → 绝对路径 → 去尾部分隔符（防止 ../ 逃逸与重复项）。</summary>
    private static bool TryNormalizeInboxPath(string? rawPath, out string fullPath)
    {
        fullPath = "";
        if (string.IsNullOrWhiteSpace(rawPath)) return false;
        var trimmed = rawPath.Trim();
        if (trimmed.Length >= 2 && trimmed[0] == '"' && trimmed[^1] == '"')
            trimmed = trimmed[1..^1].Trim();
        if (trimmed.Length == 0) return false;
        try
        {
            fullPath = Path.TrimEndingDirectorySeparator(Path.GetFullPath(trimmed));
            return true;
        }
        catch (Exception ex) when (IsIoException(ex))
        {
            return false;
        }
    }

    private static DateTime SafeGetLastWriteTimeUtc(string path)
    {
        try
        {
            return File.GetLastWriteTimeUtc(path);
        }
        catch (Exception ex) when (IsIoException(ex))
        {
            // 取不到时间时按「最新」处理：排序放最后、清理时保留（宁可留着也不误删）。
            return DateTime.MaxValue;
        }
    }

    /// <summary>收件箱日志：同时写调试输出（Trace）与调用方回调，保证「记日志」与 UI 展示两不误。</summary>
    private static void Warn(Action<string>? log, string message)
    {
        Trace.WriteLine($"[ExternalSendInbox] {message}");
        log?.Invoke(message);
    }

    /// <summary>文件系统相关的可预期异常集合（权限/占用/路径非法等，均由调用方转为中文提示而不是崩溃）。</summary>
    private static bool IsIoException(Exception ex)
        => ex is IOException or UnauthorizedAccessException or NotSupportedException
            or System.Security.SecurityException or ArgumentException;
}
