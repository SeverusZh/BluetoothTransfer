using System.Text;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class ExportServiceTests : IDisposable
{
    private readonly string _tempDir;

    public ExportServiceTests()
    {
        // 每测独立临时目录，避免相互干扰。
        _tempDir = Path.Combine(Path.GetTempPath(), $"bt_export_{Guid.NewGuid():N}");
        Directory.CreateDirectory(_tempDir);
    }

    public void Dispose()
    {
        try
        {
            if (Directory.Exists(_tempDir)) Directory.Delete(_tempDir, recursive: true);
        }
        catch
        {
            // 清理失败不影响断言
        }
    }

    /// <summary>构造一条记录，把对端名/文件名/备注等置于注入字符开头以便断言转义。</summary>
    private static TransferRecord RecordWithFields(string peerName, string fileName, string note)
        => new()
        {
            Id = 1,
            CreatedAt = "2024-01-01T00:00:00",
            Direction = TransferConst.DirRecv,
            Type = TransferConst.TypeFile,
            PeerName = peerName,
            PeerAddr = "00:11:22:33:44:55",
            Name = fileName,
            Size = 1024,
            Status = TransferConst.StatusOk,
            Channel = TransferConst.ChannelOpp,
            LocalPath = @"C:\tmp\test.bin",
            Note = note
        };

    private string TempPath(string suffix) => Path.Combine(_tempDir, $"export_{Guid.NewGuid():N}.{suffix}");

    // ---- EscapeCsv 前缀注入字符 ----

    [Theory]
    [InlineData("=SUM(A1:A9)", "'=SUM(A1:A9)")]
    [InlineData("+SUM(1,2)", "'+SUM(1,2)")]
    [InlineData("-cmd", "'-cmd")]
    [InlineData("@cmd", "'@cmd")]
    [InlineData("\tcmd", "'\tcmd")]
    [InlineData("\rcmd", "'\rcmd")]
    public void EscapeCsv_PrefixInjectionChars_PrependsQuote(string input, string expected)
    {
        var csv = ReadCsv(new[] { RecordWithFields(input, "ok.bin", "ok") });
        // PeerName 为注入字段，应被转义。
        Assert.Contains("," + expected + ",", csv);
    }

    [Fact]
    public void EscapeCsv_PrefixOnFileName_AlsoEscaped()
    {
        var csv = ReadCsv(new[] { RecordWithFields("普通设备", "=calc", "普通备注") });
        Assert.Contains("普通设备,00:11:22:33:44:55,'=calc,", csv);
    }

    // ---- EscapeCsv 保留原有逗号/引号/换行处理 ----

    [Fact]
    public void EscapeCsv_CommaIsQuoted()
    {
        var csv = ReadCsv(new[] { RecordWithFields("设备,一", "ok.bin", "ok") });
        Assert.Contains("\"设备,一\",", csv);
    }

    [Fact]
    public void EscapeCsv_EmbeddedDoubleQuoteIsDoubled()
    {
        var csv = ReadCsv(new[] { RecordWithFields("设\"备", "ok.bin", "ok") });
        Assert.Contains("\"设\"\"备\",", csv);
    }

    [Fact]
    public void EscapeCsv_NewlineIsQuoted()
    {
        var csv = ReadCsv(new[] { RecordWithFields("设备\n二", "ok.bin", "ok") });
        Assert.Contains("\"设备\n二\",", csv);
    }

    [Fact]
    public void EscapeCsv_CarriageReturnIsQuoted()
    {
        var csv = ReadCsv(new[] { RecordWithFields("设备\r三", "ok.bin", "ok") });
        Assert.Contains("\"设备\r三\",", csv);
    }

    // ---- EscapeCsv 普通文本/空/null ----

    [Fact]
    public void EscapeCsv_PlainText_Unchanged()
    {
        var csv = ReadCsv(new[] { RecordWithFields("普通设备", "file.bin", "普通备注") });
        Assert.Contains("普通设备,00:11:22:33:44:55,file.bin,", csv);
    }

    [Fact]
    public void EscapeCsv_NullAndEmpty_ProduceEmptyField()
    {
        var csv = ReadCsv(new[]
        {
            RecordWithFields(null!, "", null!)
        });
        Assert.Contains(",,", csv); // 表头为实体，字段区含空值
    }

    // ---- ExportCsv / ExportJson 端到端 ----

    [Fact]
    public void ExportCsv_WritesBomUtf8File()
    {
        var path = TempPath("csv");
        var records = new[]
        {
            RecordWithFields("设备A", "=evil.xls", "备注")
        };

        var result = ExportService.ExportCsv(records.ToList(), path);

        Assert.Equal(path, result);
        Assert.True(File.Exists(path));
        var bytes = File.ReadAllBytes(path);
        // BOM EF BB BF
        Assert.Equal(0xEF, bytes[0]);
        Assert.Equal(0xBB, bytes[1]);
        Assert.Equal(0xBF, bytes[2]);

        var text = File.ReadAllText(path, Encoding.UTF8);
        Assert.StartsWith("Id,CreatedAt,Direction,Type,PeerName,PeerAddr,Name,Size,Status,Checksum,Channel,LocalPath,Note", text);
        Assert.Contains("'=evil.xls", text);
    }

    [Fact]
    public void ExportCsv_InjectionFieldsEscapedAcrossTextAndFile()
    {
        var path = TempPath("csv");
        var csv = ReadCsv(new[]
        {
            RecordWithFields("@peer", "+file.bin", "-note")
        }, path);

        Assert.Contains("'@peer", csv);
        Assert.Contains("'+file.bin", csv);
        Assert.Contains("'-note", csv);
        // 落盘内容与内存一致
        Assert.Equal(csv, File.ReadAllText(path, Encoding.UTF8));
    }

    [Fact]
    public void ExportJson_SerializesWithoutFormulaEscaping()
    {
        var path = TempPath("json");
        var records = new[]
        {
            RecordWithFields("=SUM(1,2)", "普通.bin", "备注")
        };

        var result = ExportService.ExportJson(records.ToList(), path);

        Assert.Equal(path, result);
        Assert.True(File.Exists(path));
        var text = File.ReadAllText(path, Encoding.UTF8);
        // JSON 不应对字段加前缀单引号
        Assert.Contains("\"PeerName\": \"=SUM(1,2)\"", text);
        Assert.DoesNotContain("'=SUM(1,2)", text);
    }

    private string ReadCsv(IEnumerable<TransferRecord> records, string? path = null)
    {
        var target = path ?? TempPath("csv");
        return File.ReadAllText(ExportService.ExportCsv(records.ToList(), target), Encoding.UTF8);
    }
}
