using System.IO;
using System.Text.Json;

namespace BluetoothTransfer.Models;

public class AppConfig
{
    /// <summary>OPP 通用推送分片大小（字节）。</summary>
    public int OppChunkSize { get; set; } = 32768;
    /// <summary>OPP 连接超时（秒）。</summary>
    public int OppConnectTimeoutSeconds { get; set; } = 30;
    /// <summary>OPP 发送超时（秒）。</summary>
    public int OppSendTimeoutSeconds { get; set; } = 300;
    /// <summary>OPP 文本推送默认文件名。</summary>
    public string PushTextFileName { get; set; } = "bt-note.txt";
    /// <summary>OPP 设备要求 OBEX 认证时的密码（默认为空表示不支持认证）。</summary>
    public string OppAuthPassword { get; set; } = "";
    /// <summary>
    /// Name 头是否携带 UTF-16 BOM（0xFEFF）。多数接收端（Android/Windows）按无 BOM 解析，
    /// 32feet/Windows 原生向导均不发送 BOM；个别老式设备可能需要 BOM，可按需开启。
    /// </summary>
    public bool OppNameUseBom { get; set; } = false;
    /// <summary>
    /// OPP 连接保护级别：auto（按服务端要求，默认）/ plain（强制明文）/ encrypt（强制加密认证）。
    /// Windows 接收端（系统蓝牙文件接收向导）可能要求加密连接；Android 一般接受明文。
    /// </summary>
    public string OppProtectionLevel { get; set; } = "auto";
    /// <summary>OPP 推送失败自动重试次数（0 表示不重试）。</summary>
    public int OppRetryCount { get; set; } = 3;
    /// <summary>OPP 重试基础间隔（秒），按 1x/2x/3x 指数退避。</summary>
    public int OppRetryDelaySeconds { get; set; } = 3;

    /// <summary>内置接收助手的保存目录。</summary>
    public string ReceiveDirectory { get; set; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        "Downloads", "BluetoothReceive");

    /// <summary>内置接收助手是否每次接收前询问。</summary>
    public bool ReceiveAsk { get; set; }
    /// <summary>
    /// 发送通道模式：auto（SDP 探测，助手可用走私有协议，否则回退 OPP）/
    /// assistant（强制助手）/ opp（强制 OPP）。
    /// </summary>
    public string TransferMode { get; set; } = "auto";

    private static readonly string ConfigDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "BluetoothTransfer");
    private static readonly string ConfigPath = Path.Combine(ConfigDir, "config.json");

    public static AppConfig Load()
    {
        try
        {
            if (File.Exists(ConfigPath))
            {
                var json = File.ReadAllText(ConfigPath);
                return JsonSerializer.Deserialize<AppConfig>(json) ?? new AppConfig();
            }
        }
        catch { }
        return new AppConfig();
    }

    public void Save()
    {
        try
        {
            Directory.CreateDirectory(ConfigDir);
            var json = JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true });
            File.WriteAllText(ConfigPath, json);
        }
        catch { }
    }
}
