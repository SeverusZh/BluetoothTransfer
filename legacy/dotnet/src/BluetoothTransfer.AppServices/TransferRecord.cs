using System.Globalization;
using System.Text.Json.Serialization;

namespace BluetoothTransfer.Models;

public static class TransferConst
{
    public const string DirSend = "send";
    public const string DirRecv = "recv";
    public const string TypeText = "text";
    public const string TypeFile = "file";
    public const string TypeFolder = "folder";
    public const string StatusOk = "ok";
    public const string StatusFailed = "failed";
    public const string ChannelBle = "ble";
    public const string ChannelRfcomm = "rfcomm";
    public const string ChannelOpp = "opp";
    public const string ChannelAssistant = "assistant";
    public const string ChannelAuto = "auto";
    /// <summary>
    /// 文本记录正文入库上限（UTF-8 字节，1 MB）。超过上限时记录仍写入但不保存正文
    /// （避免 SQLite 记录体积膨胀），发送完成后复制该记录会得到「未保存正文」的中文提示。
    /// </summary>
    public const int MaxRecordContentLength = 1024 * 1024;
    // 日志级别常量（EventBus 日志事件）
    public const string LogInfo = "INFO";
    public const string LogWarn = "WARN";
    public const string LogError = "ERROR";
    public const string LogDebug = "DEBUG";
}

public class TransferRecord
{
    public long Id { get; set; }
    /// <summary>留空时由 StorageService.AddRecord 在入库时取当前时间，避免对象构造到入库间耗时导致时间戳偏早。</summary>
    public string CreatedAt { get; set; } = "";
    public string Direction { get; set; } = "send";
    public string Type { get; set; } = "text";
    public string PeerName { get; set; } = "";
    public string PeerAddr { get; set; } = "";
    public string Name { get; set; } = "";
    public long Size { get; set; }
    public string Status { get; set; } = "ok";
    public string Checksum { get; set; } = "";
    public string Channel { get; set; } = "ble";
    public string LocalPath { get; set; } = "";
    public string Note { get; set; } = "";

    /// <summary>
    /// 文本记录正文（schema v2 新增，仅 Type=text 的发送记录写入；上限见
    /// <see cref="TransferConst.MaxRecordContentLength"/>，超限时留空并在 Note 说明）。
    /// 旧库/旧记录该字段为空字符串，复制时回退读取 LocalPath 指向的文件。
    /// <para>
    /// 标注 <see cref="JsonIgnoreAttribute"/>：JSON 导出与记录投影保持 v1 的字段集与体积
    /// （正文最大 1 MB，若导出会显著放大文件且改变对外格式）。CSV 导出本就显式列举字段、不含正文。
    /// </para>
    /// </summary>
    [JsonIgnore]
    public string Content { get; set; } = "";

    /// <summary>是否保存了可复制的正文（旧记录或超过上限的文本记录为 false）。</summary>
    [JsonIgnore]
    public bool HasContent => !string.IsNullOrEmpty(Content);

    /// <summary>
    /// 是否为单文件记录：供记录表 DataTrigger 显示「复制内容」专属图标（只按 Type 判定，不涉及数据库结构）。
    /// 文件是否仍然存在由复制规划器 TransferCopyPlanner 在点击时校验并给出中文提示，避免静默失败。
    /// </summary>
    public bool IsFileRecord => Type == TransferConst.TypeFile;

    /// <summary>
    /// 供 DataGrid 展示的本地化时间（"yyyy-MM-dd HH:mm:ss"）。
    /// CreatedAt 以 ISO-8601 字符串存储，直接套用 DateTime 的 StringFormat 不会生效。
    /// </summary>
    public string CreatedAtDisplay
    {
        get
        {
            if (DateTime.TryParse(CreatedAt, CultureInfo.InvariantCulture, DateTimeStyles.RoundtripKind, out var dt))
                return dt.ToLocalTime().ToString("yyyy-MM-dd HH:mm:ss", CultureInfo.InvariantCulture);
            return CreatedAt;
        }
    }

    /// <summary>
    /// 供 DataGrid 展示的人性化大小（B / KB / MB / GB，保留 1-2 位小数）。
    /// </summary>
    public string SizeDisplay
    {
        get
        {
            const long kb = 1024L;
            const long mb = 1024L * 1024;
            const long gb = 1024L * 1024 * 1024;
            double value = Size;
            if (Size >= gb) return $"{value / gb:0.##} GB";
            if (Size >= mb) return $"{value / mb:0.##} MB";
            if (Size >= kb) return $"{value / kb:0.#} KB";
            return $"{Size} B";
        }
    }

    public string DirectionDisplay => Direction == TransferConst.DirSend ? "发送"
        : Direction == TransferConst.DirRecv ? "接收" : Direction;
    public string TypeDisplay => Type == TransferConst.TypeText ? "文本"
        : Type == TransferConst.TypeFile ? "文件"
        : Type == TransferConst.TypeFolder ? "文件夹" : Type;
    public string StatusDisplay => Status == TransferConst.StatusOk ? "成功"
        : Status == TransferConst.StatusFailed ? "失败" : Status;
    public string ChannelDisplay => Channel == TransferConst.ChannelBle ? "BLE"
        : Channel == TransferConst.ChannelRfcomm ? "RFCOMM"
        : Channel == TransferConst.ChannelOpp ? "OPP"
        : Channel == TransferConst.ChannelAssistant ? "助手" : Channel;
}
