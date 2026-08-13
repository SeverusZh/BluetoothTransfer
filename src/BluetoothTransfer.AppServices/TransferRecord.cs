using System.Globalization;

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
        : Type == TransferConst.TypeFile ? "文件" : Type;
    public string StatusDisplay => Status == TransferConst.StatusOk ? "成功"
        : Status == TransferConst.StatusFailed ? "失败" : Status;
    public string ChannelDisplay => Channel == TransferConst.ChannelBle ? "BLE"
        : Channel == TransferConst.ChannelRfcomm ? "RFCOMM"
        : Channel == TransferConst.ChannelOpp ? "OPP"
        : Channel == TransferConst.ChannelAssistant ? "助手" : Channel;
}
