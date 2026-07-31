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
        File.WriteAllText(filePath, json, new UTF8Encoding(true));
        return filePath;
    }

    private static string EscapeCsv(string value)
    {
        if (string.IsNullOrEmpty(value)) return "";
        if (value.Contains(',') || value.Contains('"') || value.Contains('\n'))
            return $"\"{value.Replace("\"", "\"\"")}\"";
        return value;
    }
}
