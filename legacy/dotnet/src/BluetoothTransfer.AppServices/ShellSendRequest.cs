using System.Text;

namespace BluetoothTransfer.Services;

/// <summary>外部发送项类型：文件 / 文件夹（zip 后发送）/ 文本正文（.txt 包装后发送）。</summary>
public enum ShellSendItemKind
{
    File,
    Folder,
    Text
}

/// <summary>
/// 单个外部发送项：路径 + 类型 +（仅文本项的）正文。
/// 选择器说明：<see cref="Path"/> 始终是「资源管理器里选中的那个路径」，文本项也不例外；
/// 这样状态提示、失败回退（文本读取失败改按文件发送）与日志都有稳定可读的来源路径。
/// </summary>
public sealed class ShellSendItem
{
    public ShellSendItemKind Kind { get; init; }

    /// <summary>规范化后的绝对路径。</summary>
    public string Path { get; init; } = "";

    /// <summary>
    /// 仅 <see cref="ShellSendItemKind.Text"/> 有值：--send-text 从文件读取的正文（UTF-8/BOM 探测）。
    /// 转发到主实例时随收件箱 JSON 一起传递；为空时由入队方重新读取文件，仍失败则回退按文件发送。
    /// </summary>
    public string Text { get; init; } = "";
}

/// <summary>
/// 文本文件读取器（--send-text 专用）：按 BOM 探测 UTF-8 / UTF-16 LE / UTF-16 BE；
/// 无 BOM 时用「偶数位/奇数位 0x00 占比」识别 UTF-16（记事本等工具的旧式输出），否则严格按 UTF-8 解码。
/// 纯 BCL、无 WPF 依赖，可在 Linux harness 上直接单测。
///
/// 安全约束：
/// 1) 仅读取普通文件，大小上限 <see cref="MaxTextBytes"/>——文本正文会内联进二次实例的转发 JSON（单文件 ≤1MB），
///    必须给序列化留出余量；超限回退为「按文件发送」，不截断（截断会静默丢内容）；
/// 2) 解码失败（二进制/其它编码）不抛异常，返回中文原因由调用方回退为按文件发送并提示；
/// 3) 文本大小以字节计，不读全文件前先看 FileInfo.Length，避免超大文件占内存。
/// </summary>
public static class ShellTextFileReader
{
    /// <summary>文本正文上限（字节）：256KB 文本经 UTF-8 序列化后仍明显小于收件箱单文件 1MB 上限。</summary>
    public const int MaxTextBytes = 256 * 1024;

    /// <summary>
    /// 读取文本文件。失败时 <paramref name="error"/> 为可直接展示的中文原因；
    /// 成功时 <paramref name="text"/> 已剥离 BOM。
    /// </summary>
    public static bool TryReadText(string? path, out string text, out string error)
    {
        text = "";
        error = "";
        if (string.IsNullOrWhiteSpace(path))
        {
            error = "文本文件路径为空";
            return false;
        }

        try
        {
            var info = new FileInfo(path);
            if (!info.Exists)
            {
                error = "文本文件不存在或已被移动";
                return false;
            }
            if (info.Length > MaxTextBytes)
            {
                error = $"文本文件过大（{info.Length:N0} 字节，上限 {MaxTextBytes:N0} 字节）";
                return false;
            }
            if (info.Length == 0)
            {
                error = "文本文件为空";
                return false;
            }

            var bytes = File.ReadAllBytes(path);
            return TryDecode(bytes, out text, out error);
        }
        catch (Exception ex) when (ex is IOException or UnauthorizedAccessException or NotSupportedException
                                       or System.Security.SecurityException)
        {
            // 典型：文件被占用、无读取权限、路径为设备名等。不抛给调用方，统一回退为按文件发送。
            error = $"读取文本文件失败：{ex.Message}";
            return false;
        }
    }

    /// <summary>按 BOM / 字节特征解码文本；解码失败返回中文原因（不抛异常）。</summary>
    private static bool TryDecode(byte[] bytes, out string text, out string error)
    {
        text = "";
        error = "";

        Encoding encoding;
        var offset = 0;
        if (bytes.Length >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF)
        {
            encoding = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);
            offset = 3;
        }
        else if (bytes.Length >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE)
        {
            encoding = new UnicodeEncoding(bigEndian: false, byteOrderMark: false, throwOnInvalidBytes: true);
            offset = 2;
        }
        else if (bytes.Length >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF)
        {
            encoding = new UnicodeEncoding(bigEndian: true, byteOrderMark: false, throwOnInvalidBytes: true);
            offset = 2;
        }
        else if (LooksLikeUtf16(bytes, out var bigEndian))
        {
            encoding = new UnicodeEncoding(bigEndian, byteOrderMark: false, throwOnInvalidBytes: true);
        }
        else
        {
            encoding = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);
        }

        try
        {
            text = encoding.GetString(bytes, offset, bytes.Length - offset);
        }
        catch (Exception ex) when (ex is DecoderFallbackException or ArgumentException)
        {
            // 二进制文件或 GBK/ANSI 等其它编码：无法安全当作文本发送，交由调用方回退为按文件发送。
            error = "文件内容不是有效的 UTF-8/UTF-16 文本（可能为二进制或其它编码）";
            return false;
        }

        if (text.Length == 0)
        {
            error = "文本文件为空";
            return false;
        }
        if (text.Contains('\0'))
        {
            error = "文件内容包含 NUL 字符（可能是二进制文件）";
            return false;
        }
        return true;
    }

    /// <summary>无 BOM 时的 UTF-16 探测：ASCII 为主的文本中有一半字节是 0x00，且集中在同一侧位置。</summary>
    private static bool LooksLikeUtf16(byte[] bytes, out bool bigEndian)
    {
        bigEndian = false;
        if (bytes.Length < 4 || bytes.Length % 2 != 0) return false;

        var samples = Math.Min(bytes.Length, 4096);
        var evenZero = 0;
        var oddZero = 0;
        for (var i = 0; i < samples; i++)
        {
            if (bytes[i] != 0) continue;
            if ((i & 1) == 0) evenZero++;
            else oddZero++;
        }

        // 判定条件：某一侧 0x00 占比 ≥40%，另一侧 ≤10%（UTF-16 LE 的 ASCII 字符高字节为 0，落在奇数位）。
        // 两侧都要求避免把「恰好含少量 0x00 的二进制/UTF-8 文件」误判成 UTF-16。
        var half = samples / 2;
        if (half == 0) return false;
        var oddRatio = (double)oddZero / half;
        var evenRatio = (double)evenZero / half;
        if (oddRatio >= 0.4 && evenRatio <= 0.1)
        {
            bigEndian = false; // UTF-16 LE：低字节在前，ASCII 字符的 0x00 落在奇数位
            return true;
        }
        if (evenRatio >= 0.4 && oddRatio <= 0.1)
        {
            bigEndian = true; // UTF-16 BE：高字节在前，ASCII 字符的 0x00 落在偶数位
            return true;
        }
        return false;
    }
}

/// <summary>
/// Shell 命令行/注册表命令模板的单一事实源：资源管理器右键菜单安装（<c>ExplorerIntegrationService</c>）
/// 与 <c>--send</c>/<c>--send-text</c> 解析共用同一份参数名与引号规则，避免「装进注册表的命令」
/// 与「应用实际解析的参数」漂移。全部为纯字符串拼接，可在 Linux harness 上单测。
/// </summary>
public static class ShellSendSyntax
{
    /// <summary>资源管理器右键菜单/SendTo 传入「文件或文件夹路径」的参数名。</summary>
    public const string SendOption = "--send";

    /// <summary>资源管理器右键菜单传入「文本文件路径（读取正文后按文本发送）」的参数名。</summary>
    public const string SendTextOption = "--send-text";

    /// <summary>「发送到」目录中快捷方式的显示名（中文，与安装结果一致）。</summary>
    public const string SendToShortcutFileName = "蓝牙传输.lnk";

    /// <summary>SendTo 快捷方式的 Arguments：只写 --send，选中项路径由资源管理器自动追加并逐项处理。</summary>
    public const string SendToArguments = SendOption;

    /// <summary>
    /// 给一个命令行参数整体加双引号。Windows 文件路径不允许包含双引号字符，因此无需再转义；
    /// 注册表命令里的 <c>%1</c> / <c>%V</c> 占位符同样要整体加引号（路径含空格/中文时才能正确拆分）。
    /// </summary>
    public static string QuoteArgument(string value)
    {
        ArgumentNullException.ThrowIfNull(value);
        return "\"" + value + "\"";
    }

    /// <summary>构造文件/文件夹右键菜单命令：<c>"&lt;exe&gt;" --send "&lt;placeholder&gt;"</c>。</summary>
    public static string BuildSendCommand(string exePath, string placeholder)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(exePath);
        ArgumentException.ThrowIfNullOrWhiteSpace(placeholder);
        return $"{QuoteArgument(exePath)} {SendOption} {QuoteArgument(placeholder)}";
    }

    /// <summary>构造文本文件右键菜单命令：<c>"&lt;exe&gt;" --send-text "&lt;placeholder&gt;"</c>。</summary>
    public static string BuildSendTextCommand(string exePath, string placeholder)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(exePath);
        ArgumentException.ThrowIfNullOrWhiteSpace(placeholder);
        return $"{QuoteArgument(exePath)} {SendTextOption} {QuoteArgument(placeholder)}";
    }

    /// <summary>构造注册表 Icon 值：<c>"&lt;exe&gt;",0</c>（复用应用图标）。</summary>
    public static string BuildIconValue(string exePath)
    {
        ArgumentException.ThrowIfNullOrWhiteSpace(exePath);
        return $"{QuoteArgument(exePath)},0";
    }
}

/// <summary>
/// 资源管理器右键菜单 / 「发送到」/ 二次实例转发的发送请求（纯逻辑、无 WPF 依赖，可单测）。
///
/// 命令行契约（由 <see cref="ShellSendSyntax"/> 统一维护）：
/// <code>
/// BluetoothTransfer.exe --send "C:\a\one.txt" "C:\data\docs"
/// BluetoothTransfer.exe --send-text "C:\notes\note.txt"
/// </code>
///
/// 规则：
/// 1) <c>--send</c> 后的全部剩余非选项参数按路径解析；<c>--send-text</c> 取紧随其后的一个文件；
/// 2) 目录 → <see cref="ShellSendItemKind.Folder"/>（zip 后发送），文件 → <see cref="ShellSendItemKind.File"/>；
/// 3) 只接受已存在的路径；大小写不敏感去重（<see cref="Path.GetFullPath"/> 规范化后比较）；
/// 4) 一次最多 <see cref="MaxItems"/> 项，超出部分忽略并给出提示；
/// 5) 无参数 / 未知参数返回空请求（GUI 正常启动）；解析过程中的非致命问题全部记入 <see cref="Warnings"/>，
///    由调用方经状态栏/日志展示，不静默、也不弹错误框打断正常启动。
/// </summary>
public sealed class ShellSendRequest
{
    /// <summary>单次请求最多处理的项数（静态 Verb 多选/SendTo 命令行长度与队列压力上限）。</summary>
    public const int MaxItems = 100;

    private readonly ShellSendItem[] _items;
    private readonly string[] _warnings;

    private ShellSendRequest(ShellSendItem[] items, string[] warnings)
    {
        _items = items;
        _warnings = warnings;
    }

    /// <summary>空请求（双击启动/无参数启动）。</summary>
    public static ShellSendRequest Empty { get; } = new(Array.Empty<ShellSendItem>(), Array.Empty<string>());

    /// <summary>解析出的待发送项（只读）。</summary>
    public IReadOnlyList<ShellSendItem> Items => _items;

    /// <summary>解析阶段的非致命提示（路径失效、文本回退、超出上限等），可直接展示给用户。</summary>
    public IReadOnlyList<string> Warnings => _warnings;

    public bool HasItems => _items.Length > 0;

    public int Count => _items.Length;

    /// <summary>
    /// 解析命令行参数（通常是 WPF 的 <c>StartupEventArgs.Args</c>，已经过系统拆分，不需要再处理引号）。
    /// 未知参数被忽略，保证从命令行/快捷方式带其它参数启动时仍能正常打开 GUI。
    /// </summary>
    public static ShellSendRequest Parse(IEnumerable<string>? args)
    {
        if (args == null) return Empty;
        var tokens = args.Where(t => !string.IsNullOrEmpty(t)).ToList();
        if (tokens.Count == 0) return Empty;

        var items = new List<ShellSendItem>();
        var warnings = new List<string>();
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var limitWarned = false;

        void WarnLimitOnce()
        {
            if (limitWarned) return;
            limitWarned = true;
            warnings.Add($"一次最多处理 {MaxItems} 项，其余项已忽略");
        }

        var i = 0;
        while (i < tokens.Count)
        {
            var token = tokens[i];

            if (string.Equals(token, ShellSendSyntax.SendOption, StringComparison.OrdinalIgnoreCase))
            {
                i++;
                var pathCount = 0;
                while (i < tokens.Count && !IsOptionToken(tokens[i]))
                {
                    if (items.Count >= MaxItems)
                    {
                        // 已到上限：跳过本组剩余的路径参数，给出一次提示后继续解析后续选项（如 --send-text）。
                        WarnLimitOnce();
                        while (i < tokens.Count && !IsOptionToken(tokens[i])) i++;
                        break;
                    }
                    AddPathItem(tokens[i], items, warnings, seen);
                    pathCount++;
                    i++;
                }
                if (pathCount == 0 && !limitWarned)
                    warnings.Add($"{ShellSendSyntax.SendOption} 后缺少路径参数，已忽略");
                continue;
            }

            if (string.Equals(token, ShellSendSyntax.SendTextOption, StringComparison.OrdinalIgnoreCase))
            {
                i++;
                if (i >= tokens.Count || IsOptionToken(tokens[i]))
                {
                    warnings.Add($"{ShellSendSyntax.SendTextOption} 后缺少文本文件参数，已忽略");
                    continue;
                }
                if (items.Count >= MaxItems)
                {
                    WarnLimitOnce();
                    i++;
                    continue;
                }
                AddTextItem(tokens[i], items, warnings, seen);
                i++;
                continue;
            }

            // 未知选项/位置参数：GUI 正常启动时其它启动参数不应影响启动，只记录忽略（有发送项时才展示）。
            if (IsOptionToken(token)) warnings.Add($"已忽略未知参数：{token}");
            i++;
        }

        return FromItems(items, warnings);
    }

    /// <summary>
    /// 合并多个请求（如：启动时自身命令行 + 收件箱残留 + 二次实例转发的去抖批次）：
    /// 按路径大小写不敏感去重、总数不超过 <see cref="MaxItems"/>、汇总提示信息。
    /// </summary>
    public static ShellSendRequest Merge(IEnumerable<ShellSendRequest?>? requests)
    {
        if (requests == null) return Empty;

        var items = new List<ShellSendItem>();
        var warnings = new List<string>();
        var seen = new HashSet<string>(StringComparer.OrdinalIgnoreCase);
        var limitWarned = false;

        foreach (var request in requests)
        {
            if (request == null) continue;
            warnings.AddRange(request.Warnings);
            foreach (var item in request.Items)
            {
                if (items.Count >= MaxItems)
                {
                    if (!limitWarned)
                    {
                        limitWarned = true;
                        warnings.Add($"一次最多处理 {MaxItems} 项，其余项已忽略");
                    }
                    continue;
                }
                var key = NormalizePathKey(item.Path);
                if (key == null || !seen.Add(key)) continue;
                items.Add(item);
            }
        }

        return FromItems(items, warnings);
    }

    /// <summary>由发送项与提示信息构造请求（供收件箱排空/合并使用）；自动截断到 <see cref="MaxItems"/>。</summary>
    public static ShellSendRequest FromItems(IEnumerable<ShellSendItem>? items, IEnumerable<string>? warnings = null)
    {
        var itemList = items?.OfType<ShellSendItem>()
            .Where(i => !string.IsNullOrWhiteSpace(i.Path))
            .Take(MaxItems)
            .ToArray() ?? Array.Empty<ShellSendItem>();
        var warningList = warnings?.Where(w => !string.IsNullOrWhiteSpace(w)).ToArray() ?? Array.Empty<string>();
        return itemList.Length == 0 && warningList.Length == 0
            ? Empty
            : new ShellSendRequest(itemList, warningList);
    }

    /// <summary>把单个路径解析为发送项：存在性校验 + 目录/文件分类 + 去重。</summary>
    private static void AddPathItem(string rawPath, List<ShellSendItem> items, List<string> warnings, HashSet<string> seen)
    {
        if (!TryNormalizePath(rawPath, out var fullPath, out var pathError))
        {
            warnings.Add($"已忽略无效路径「{rawPath}」：{pathError}");
            return;
        }
        if (!seen.Add(fullPath))
        {
            // 去重静默处理：多选时 Shell 可能把同一路径重复传入，重复提示只会制造噪音。
            return;
        }

        if (Directory.Exists(fullPath))
        {
            items.Add(new ShellSendItem { Kind = ShellSendItemKind.Folder, Path = fullPath });
            return;
        }
        if (File.Exists(fullPath))
        {
            items.Add(new ShellSendItem { Kind = ShellSendItemKind.File, Path = fullPath });
            return;
        }

        seen.Remove(fullPath);
        warnings.Add($"路径不存在或不可访问，已忽略：{fullPath}");
    }

    /// <summary>把 --send-text 的文本文件解析为发送项；读取失败按需求回退为「按文件发送」并提示。</summary>
    private static void AddTextItem(string rawPath, List<ShellSendItem> items, List<string> warnings, HashSet<string> seen)
    {
        if (!TryNormalizePath(rawPath, out var fullPath, out var pathError))
        {
            warnings.Add($"已忽略无效文本路径「{rawPath}」：{pathError}");
            return;
        }
        if (!seen.Add(fullPath)) return;

        if (!File.Exists(fullPath))
        {
            seen.Remove(fullPath);
            warnings.Add($"文本文件不存在或不可访问，已忽略：{fullPath}");
            return;
        }

        var fileName = Path.GetFileName(fullPath);
        if (ShellTextFileReader.TryReadText(fullPath, out var text, out var readError))
        {
            items.Add(new ShellSendItem { Kind = ShellSendItemKind.Text, Path = fullPath, Text = text });
            return;
        }

        // 回退：按普通文件发送，保留原路径与文件名，并在提示里说明原因（用户可选其它方式重试）。
        items.Add(new ShellSendItem { Kind = ShellSendItemKind.File, Path = fullPath });
        warnings.Add($"读取文本失败（{readError}），已改为按文件发送：{fileName}");
    }

    /// <summary>选项 token 判定：以 -- 开头视为选项（路径几乎不可能以 -- 开头，且解析必须能识别后续选项）。</summary>
    private static bool IsOptionToken(string token) => token.StartsWith("--", StringComparison.Ordinal);

    /// <summary>路径规范化：去两端空白/多余引号 → 绝对路径 → 去掉末尾目录分隔符（供存在性判断与去重）。</summary>
    private static bool TryNormalizePath(string rawPath, out string fullPath, out string error)
    {
        fullPath = "";
        error = "";
        if (string.IsNullOrWhiteSpace(rawPath))
        {
            error = "路径为空";
            return false;
        }

        var trimmed = rawPath.Trim();
        // Shell 正常会剥掉引号；这里再兜底一次手工粘贴的 "C:\a b\c.txt" 形式。
        if (trimmed.Length >= 2 && trimmed[0] == '"' && trimmed[^1] == '"')
            trimmed = trimmed[1..^1].Trim();
        if (trimmed.Length == 0)
        {
            error = "路径为空";
            return false;
        }

        try
        {
            fullPath = Path.TrimEndingDirectorySeparator(Path.GetFullPath(trimmed));
            return true;
        }
        catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException
                                       or System.Security.SecurityException)
        {
            error = ex.Message;
            return false;
        }
    }

    /// <summary>去重键：绝对路径 + 去尾部分隔符，大小写不敏感。</summary>
    private static string? NormalizePathKey(string path)
    {
        if (string.IsNullOrWhiteSpace(path)) return null;
        try
        {
            return Path.TrimEndingDirectorySeparator(Path.GetFullPath(path));
        }
        catch (Exception ex) when (ex is ArgumentException or NotSupportedException or PathTooLongException
                                       or System.Security.SecurityException)
        {
            return null;
        }
    }
}
