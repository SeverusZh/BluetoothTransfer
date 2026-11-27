using System.IO;
using System.Text;
using System.Text.Json;
using BluetoothTransfer.Models;

namespace BluetoothTransfer.Services;

public class ExportService
{
    public static string ExportCsv(List<TransferRecord> records, string filePath)
    {
        var sb = new StringBuilder();
        sb.AppendLine("Id,CreatedAt,Direction,Type,PeerName,PeerAddr,Name,Size,Status,Checksum,Channel,LocalPath,Note");
        foreach (var r in records)
        {
            sb.AppendLine(string.Join(",",
                r.Id,
                EscapeCsv(r.CreatedAt),
                r.Direction,
                r.Type,
                EscapeCsv(r.PeerName),
                EscapeCsv(r.PeerAddr),
                EscapeCsv(r.Name),
                r.Size,
                r.Status,
                EscapeCsv(r.Checksum),
                r.Channel,
                EscapeCsv(r.LocalPath),
                EscapeCsv(r.Note)));
        }
        // 带 BOM 的 UTF-8，便于 Excel 直接识别中文表头。
        File.WriteAllText(filePath, sb.ToString(), new UTF8Encoding(true));
        return filePath;
    }

    public static string ExportJson(List<TransferRecord> records, string filePath)
    {
        var json = JsonSerializer.Serialize(records, new JsonSerializerOptions { WriteIndented = true });
        // JSON 文本按 RFC 8259 不得带 BOM，仅 CSV 需要 BOM 以兼容 Excel。
        File.WriteAllText(filePath, json, Encoding.UTF8);
        return filePath;
    }

    // 对端设备名/文件名等字段可来自外部，开头的 = + - @ \t \r 在 Excel 打开时可能触发公式注入，
    // 需统一加前缀单引号 ' 将其转成纯文本；\r 同时纳入需要引号包裹的判断。
    private static string EscapeCsv(string value)
    {
        if (string.IsNullOrEmpty(value)) return "";
        // 以 =+-\t\r 开头的字段可能被 Excel 当作公式执行，统一加前缀单引号转纯文本（含 \r）。
        if (value[0] == '=' || value[0] == '+' || value[0] == '-' || value[0] == '@'
            || value[0] == '\t' || value[0] == '\r')
            return "'" + value;
        if (value.Contains(',') || value.Contains('"') || value.Contains('\n') || value.Contains('\r'))
            return $"\"{value.Replace("\"", "\"\"")}\"";
        return value;
    }
}
