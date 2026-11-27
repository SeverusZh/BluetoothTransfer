using System.IO;
using System.Text;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

/// <summary>复制到剪贴板的目标类型。</summary>
public enum TransferCopyKind
{
    /// <summary>失败：没有可复制的目标。</summary>
    None = 0,

    /// <summary>文本：<see cref="TransferCopyPlan.Payload"/> 为文本正文。</summary>
    Text,

    /// <summary>图片：<see cref="TransferCopyPlan.Payload"/> 为本地图片路径（由 WPF 适配器解码成位图后写入剪贴板）。</summary>
    Image,

    /// <summary>文件/文件夹：<see cref="TransferCopyPlan.Payload"/> 为本地绝对路径（文件剪贴板）。</summary>
    File
}

/// <summary>
/// 复制规划结果（纯数据、不依赖 WPF）：成功时 <see cref="Kind"/>/<see cref="Payload"/> 描述「往剪贴板放什么」，
/// 失败时 <see cref="Error"/> 给出可直接展示在状态栏的中文原因，调用方据此上报日志，避免静默失败。
/// </summary>
public sealed class TransferCopyPlan
{
    private TransferCopyPlan(TransferCopyKind kind, string payload, string error, string note)
    {
        Kind = kind;
        Payload = payload;
        Error = error;
        Note = note;
    }

    public TransferCopyKind Kind { get; }

    /// <summary>Text=文本正文；Image/File=本地绝对路径；失败时为空字符串。</summary>
    public string Payload { get; }

    /// <summary>失败时的中文原因；成功时为空字符串。</summary>
    public string Error { get; }

    /// <summary>成功时的补充说明（如「该格式不支持文本/图片内容提取」），无补充时为空字符串。</summary>
    public string Note { get; }

    public bool IsSuccess => Kind != TransferCopyKind.None && Error.Length == 0;

    public static TransferCopyPlan Text(string text) => new(TransferCopyKind.Text, text ?? "", "", "");

    public static TransferCopyPlan Image(string path) => new(TransferCopyKind.Image, path, "", "");

    public static TransferCopyPlan File(string path, string note = "") => new(TransferCopyKind.File, path, "", note);

    public static TransferCopyPlan Fail(string reason) => new(TransferCopyKind.None, "", reason, "");
}

/// <summary>
/// 传输记录「复制」纯逻辑规划器：只做路径/扩展名/存在性判断与有大小上限的文本读取，不依赖 WPF，便于单元测试。
/// 两类入口对应界面上两个图标：
/// 1. <see cref="PlanCopy"/> 通用复制：文本记录→正文（优先 <see cref="TransferRecord.Content"/>，
///    旧记录回退读取 LocalPath 文件）；图片文件→图片；其它文件/文件夹→文件剪贴板；
/// 2. <see cref="PlanContentCopy"/> 单文件专属「复制内容」：文本类扩展名→文本；图片扩展名→图片；
///    其余格式回退复制文件本身（<see cref="TransferCopyPlan.Note"/> 说明原因）；文件缺失/无路径时返回中文 Error。
/// 真正的图片解码与剪贴板写入由 GUI 层 WpfClipboardWriter 完成，本类可在后台线程安全调用。
/// </summary>
public static class TransferCopyPlanner
{
    /// <summary>
    /// 文本内容读取上限（8 MB）：超过上限直接返回中文原因而不读取，避免把超大文件读进内存/剪贴板。
    /// </summary>
    public const long MaxTextBytes = 8L * 1024 * 1024;

    /// <summary>
    /// 图片文件解码上限（64 MB）：超过上限不做图片解码，避免占用大量内存（解码后的位图通常远大于文件本身）。
    /// </summary>
    public const long MaxImageBytes = 64L * 1024 * 1024;

    /// <summary>按扩展名视为「文本内容」的格式：复制内容时读取正文，而不是把文件本身放进剪贴板。</summary>
    private static readonly HashSet<string> TextExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".txt", ".md", ".markdown", ".log", ".json", ".csv", ".tsv", ".xml", ".ini", ".yml", ".yaml",
        ".config", ".conf", ".toml", ".properties", ".sql", ".html", ".htm", ".css", ".js", ".mjs",
        ".ts", ".cs", ".py", ".java", ".go", ".rs", ".c", ".h", ".cpp", ".hpp", ".sh", ".bat", ".cmd",
        ".ps1"
    };

    /// <summary>按扩展名视为「图片」的格式（能否解码由平台图片编解码器决定，解码失败由适配器上报中文错误）。</summary>
    private static readonly HashSet<string> ImageExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".webp", ".ico", ".tif", ".tiff"
    };

    /// <summary>
    /// UTF-32 大端解码器：Encoding.UTF32 固定为小端，BOM 为 00 00 FE FF 时必须用大端实例，
    /// 否则「Hi」之类内容会被按小端解读成乱码。
    /// </summary>
    private static readonly UTF32Encoding BigEndianUtf32 = new(bigEndian: true, byteOrderMark: false);

    /// <summary>
    /// 通用复制规划：把「传输对象」放进剪贴板。
    /// 文本记录优先使用随记录保存的正文（<see cref="TransferRecord.Content"/>，schema v2 起写入）；
    /// 无正文的旧记录回退读取 LocalPath 指向的文本文件；
    /// 文件夹/普通文件→文件剪贴板；图片文件→图片。
    /// </summary>
    public static TransferCopyPlan PlanCopy(TransferRecord? record, long maxTextBytes = MaxTextBytes)
    {
        if (record == null) return TransferCopyPlan.Fail("复制失败：未指定传输记录");

        // schema v2：发送文本记录直接携带正文，不再依赖发送后即被清理的临时 .txt。
        if (record.Type == TransferConst.TypeText && record.HasContent)
        {
            var contentBytes = Encoding.UTF8.GetByteCount(record.Content);
            if (contentBytes > maxTextBytes) return TooLargePlan("文本", contentBytes, maxTextBytes);
            return TransferCopyPlan.Text(record.Content);
        }

        if (!TryResolvePath(record, "无法复制", out var path, out var error))
        {
            // 文本记录可能既无正文（旧版本记录/正文超过入库上限）又没有可读的本地文件：
            // 给出可操作的中文说明，而不是笼统的“文件不存在”。
            return record.Type == TransferConst.TypeText && !record.HasContent
                ? TransferCopyPlan.Fail($"{error}；该文本记录未保存正文（旧版本记录，或正文超过 " +
                    $"{FormatSize(TransferConst.MaxRecordContentLength)} 入库上限）")
                : TransferCopyPlan.Fail(error);
        }
        // 文件夹任务记录（或指向目录的路径）直接进文件剪贴板：资源管理器可粘贴出整个文件夹。
        if (Directory.Exists(path)) return TransferCopyPlan.File(path);
        // 文本记录按正文复制；其余文件记录保持「文件」语义（.txt 文件记录也仍是文件，只有 Type=text 才是文本）。
        if (record.Type == TransferConst.TypeText) return ReadTextPlan(path, maxTextBytes);
        return IsImageFile(path) ? BuildImagePlan(path) : TransferCopyPlan.File(path);
    }

    /// <summary>
    /// 单文件专属「复制内容」规划：读取文件内容放入剪贴板。
    /// 文本类扩展名→文本；图片扩展名→图片；其余格式回退复制文件本身（Note 说明原因）。
    /// 仅 Type=file 的记录支持；文件夹、缺失文件、无本地路径均返回中文原因。
    /// </summary>
    public static TransferCopyPlan PlanContentCopy(TransferRecord? record, long maxTextBytes = MaxTextBytes)
    {
        if (record == null) return TransferCopyPlan.Fail("复制失败：未指定传输记录");
        if (!record.IsFileRecord) return TransferCopyPlan.Fail("仅文件类型的记录支持复制内容");
        if (!TryResolvePath(record, "无法复制内容", out var path, out var error)) return TransferCopyPlan.Fail(error);
        if (Directory.Exists(path)) return TransferCopyPlan.Fail("该记录指向文件夹，不支持复制内容");

        var extension = Path.GetExtension(path);
        if (TextExtensions.Contains(extension)) return ReadTextPlan(path, maxTextBytes);
        if (ImageExtensions.Contains(extension)) return BuildImagePlan(path);
        return TransferCopyPlan.File(path, "该格式不支持文本/图片内容提取");
    }

    /// <summary>解析记录本地路径为绝对路径并校验存在性；失败时返回可直接展示的中文原因。</summary>
    private static bool TryResolvePath(TransferRecord record, string action, out string fullPath, out string error)
    {
        fullPath = "";
        if (string.IsNullOrWhiteSpace(record.LocalPath))
        {
            error = $"该记录没有本地文件路径，{action}";
            return false;
        }
        try
        {
            fullPath = Path.GetFullPath(record.LocalPath);
        }
        catch (Exception ex)
        {
            error = $"本地路径无效，{action}：{ex.Message}";
            return false;
        }
        if (!File.Exists(fullPath) && !Directory.Exists(fullPath))
        {
            // 典型场景：发出的文本记录 LocalPath 指向发送后即被清理的临时 .txt，属于既有数据模型限制。
            error = $"本地文件已不存在（可能已被移动、删除或原本为临时文件），{action}";
            return false;
        }
        error = "";
        return true;
    }

    /// <summary>读取文本文件为文本剪贴板内容；先按文件大小拦截上限，读取后再按实际字节数兜底校验。</summary>
    private static TransferCopyPlan ReadTextPlan(string path, long maxTextBytes)
    {
        long length;
        try
        {
            length = new FileInfo(path).Length;
        }
        catch (Exception ex)
        {
            return TransferCopyPlan.Fail($"读取文件信息失败：{ex.Message}");
        }
        if (length > maxTextBytes) return TooLargePlan("文本", length, maxTextBytes);

        byte[] bytes;
        try
        {
            bytes = File.ReadAllBytes(path);
        }
        catch (Exception ex)
        {
            return TransferCopyPlan.Fail($"读取文本文件失败：{ex.Message}");
        }
        // 兜底：文件在检查与读取之间变大时不把超限内容放进剪贴板。
        if (bytes.Length > maxTextBytes) return TooLargePlan("文本", bytes.Length, maxTextBytes);
        return TransferCopyPlan.Text(DecodeText(bytes));
    }

    /// <summary>图片规划：先按文件大小拦截解码上限（解码由 WPF 适配器完成），Payload 为本地路径。</summary>
    private static TransferCopyPlan BuildImagePlan(string path)
    {
        try
        {
            var length = new FileInfo(path).Length;
            if (length > MaxImageBytes) return TooLargePlan("图片", length, MaxImageBytes);
        }
        catch (Exception ex)
        {
            return TransferCopyPlan.Fail($"读取图片信息失败：{ex.Message}");
        }
        return TransferCopyPlan.Image(path);
    }

    private static TransferCopyPlan TooLargePlan(string kind, long size, long limit)
        => TransferCopyPlan.Fail($"{kind}文件过大（{FormatSize(size)}），超过 {FormatSize(limit)} 的复制上限，未读取内容");

    private static bool IsImageFile(string path) => ImageExtensions.Contains(Path.GetExtension(path));

    private static string FormatSize(long size)
    {
        const long kb = 1024L;
        const long mb = 1024L * 1024;
        const long gb = 1024L * 1024 * 1024;
        double value = size;
        if (size >= gb) return $"{value / gb:0.##} GB";
        if (size >= mb) return $"{value / mb:0.##} MB";
        if (size >= kb) return $"{value / kb:0.#} KB";
        return $"{size} B";
    }

    /// <summary>
    /// 文本解码：识别 UTF-8/UTF-16/UTF-32 BOM；无 BOM 时优先按严格 UTF-8 解码，
    /// 非法字节序列回退 Encoding.Latin1（逐字节 1:1 映射，跨平台确定；非 UTF-8 旧文本会显示为乱码但可按字节还原）。
    /// </summary>
    private static string DecodeText(byte[] bytes)
    {
        if (bytes.Length == 0) return "";
        // BOM 判定顺序：UTF-32 LE 的 FF FE 00 00 与 UTF-16 LE 的 FF FE 前缀重叠，必须先判 UTF-32。
        // 注意 UTF-32 大/小端必须分别解码：Encoding.UTF32 固定为小端，不能用于 00 00 FE FF 大端 BOM。
        if (HasPrefix(bytes, 0x00, 0x00, 0xFE, 0xFF)) return BigEndianUtf32.GetString(bytes, 4, bytes.Length - 4);
        if (HasPrefix(bytes, 0xFF, 0xFE, 0x00, 0x00)) return Encoding.UTF32.GetString(bytes, 4, bytes.Length - 4);
        if (HasPrefix(bytes, 0xEF, 0xBB, 0xBF)) return Encoding.UTF8.GetString(bytes, 3, bytes.Length - 3);
        if (HasPrefix(bytes, 0xFE, 0xFF)) return Encoding.BigEndianUnicode.GetString(bytes, 2, bytes.Length - 2);
        if (HasPrefix(bytes, 0xFF, 0xFE)) return Encoding.Unicode.GetString(bytes, 2, bytes.Length - 2);
        try
        {
            return new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true).GetString(bytes);
        }
        catch (DecoderFallbackException)
        {
            // 严格 UTF-8 解码失败说明内容不是合法 UTF-8（可能是损坏文件或其它编码）。
            // 回退 Encoding.Latin1：按字节 1:1 映射为 U+0000-U+00FF，跨平台行为确定且不产生替换字符
            //（内容仍可按字节还原）。注意 .NET 8 的 Encoding.Default 就是 UTF-8，并非 Windows ANSI 代码页，
            // 因此不能拿它来兼容 GBK/ANSI 旧文本。
            return Encoding.Latin1.GetString(bytes);
        }
    }

    private static bool HasPrefix(byte[] bytes, params byte[] prefix)
    {
        if (bytes.Length < prefix.Length) return false;
        for (var i = 0; i < prefix.Length; i++)
        {
            if (bytes[i] != prefix[i]) return false;
        }
        return true;
    }
}
