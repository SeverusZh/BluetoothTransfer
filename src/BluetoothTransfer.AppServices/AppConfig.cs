using System.IO;
using System.Text.Json;
using BluetoothTransfer.Core.Protocol;

namespace BluetoothTransfer.Models;

public class AppConfig
{
    /// <summary>
    /// OPP 通用推送分片大小（字节）。默认 32768：真实接收端（Android/Windows/老式车机）的
    /// OBEX 最大包长协商并不总是可靠，增大分片存在兼容性风险，故默认值保持不变；
    /// 实测对端支持大包时可调到 65536 再验证。
    /// </summary>
    public int OppChunkSize { get; set; } = 32768;
    /// <summary>OPP 连接超时（秒）。只覆盖连接建立阶段，与发送阶段超时相互独立。</summary>
    public int OppConnectTimeoutSeconds { get; set; } = 30;
    /// <summary>
    /// OPP 发送<b>空闲</b>超时（秒）：连续无进展（读取下一分片/等待对端响应）超过该时长才判定超时。
    /// 注意语义已在第二阶段从「整次发送总时长上限」改为「空闲超时」：大文件只要持续有进展就不会
    /// 在 300 秒处被整体掐断，而卡死的对端仍会在该时长后失败。
    /// </summary>
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
    /// <summary>OPP 重试基础间隔（秒），按 1x/2x/4x 指数退避。</summary>
    public int OppRetryDelaySeconds { get; set; } = 3;

    /// <summary>助手通道默认分片大小（字节）：256 KB（旧版为 32 KB）。</summary>
    public const int DefaultAssistantChunkSize = 256 * 1024;

    /// <summary>助手通道分片下限（字节）：过小的分片会显著增加帧/ACK 往返次数。</summary>
    public const int MinAssistantChunkSize = 4096;

    /// <summary>
    /// 助手通道分片上限（字节）：单帧上限扣除帧头（<see cref="AsstFrame.HeaderSize"/>）
    /// 与 DATA 载荷内的偏移(8)/长度(4)字段（共 12 字节），确保整帧不超过 <see cref="AsstConst.MaxFrameSize"/>。
    /// </summary>
    public static int MaxAssistantChunkSize => AsstConst.MaxFrameSize - AsstFrame.HeaderSize - 12;

    private int _assistantChunkSize = DefaultAssistantChunkSize;

    /// <summary>
    /// 助手通道 HELLO 声明的分片大小（字节）。默认 256 KB：助手协议为单帧 ≤1 MB 的私有分片协议，
    /// 大分片可直接减少 DATA/ACK 往返次数（蓝牙 RFCOMM 上主要收益来源）；旧接收端只校验帧上限，
    /// 故 256 KB 向后兼容。越界值（含配置文件手改）自动钳制到 [4 KB, 单帧上限-帧头-12]。
    /// </summary>
    public int AssistantChunkSize
    {
        get => _assistantChunkSize;
        set => _assistantChunkSize = Math.Clamp(value, MinAssistantChunkSize, MaxAssistantChunkSize);
    }

    /// <summary>内置接收助手的保存目录。</summary>
    public string ReceiveDirectory { get; set; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        "Downloads", "BluetoothReceive");

    /// <summary>内置接收助手是否每次接收前询问（默认开启，防止已配对设备直接落盘）。</summary>
    public bool ReceiveAsk { get; set; } = true;
    /// <summary>
    /// 发送通道模式：auto（SDP 探测，助手可用走私有协议，否则回退 OPP）/
    /// assistant（强制助手）/ opp（强制 OPP）。
    /// </summary>
    public string TransferMode { get; set; } = "auto";

    /// <summary>最近一次配置读写错误摘要；成功时为空，失败时记录中文错误信息（便于 GUI 上报）。不参与 JSON 序列化。</summary>
    [System.Text.Json.Serialization.JsonIgnore]
    public string? LastError { get; private set; }

    private static readonly string ConfigDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "BluetoothTransfer");
    private static readonly string ConfigPath = Path.Combine(ConfigDir, "config.json");

    public static AppConfig Load()
    {
        var cfg = new AppConfig();
        try
        {
            if (File.Exists(ConfigPath))
            {
                var json = File.ReadAllText(ConfigPath);
                cfg = JsonSerializer.Deserialize<AppConfig>(json) ?? new AppConfig();
            }
            cfg.LastError = null;
        }
        catch (Exception ex)
        {
            // 磁盘/权限/JSON 损坏异常不再静默吞掉，记录错误摘要供 GUI 上报；
            // 即便失败仍返回默认配置对象，保证解析方拿到可用的实例。
            cfg.LastError = $"读取配置文件失败：{ex.Message}";
        }
        return cfg;
    }

    public void Save()
    {
        try
        {
            Directory.CreateDirectory(ConfigDir);
            var json = JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true });
            File.WriteAllText(ConfigPath, json);
            LastError = null;
        }
        catch (Exception ex)
        {
            // 磁盘满/权限问题不再静默吞掉，记录错误摘要供 GUI 上报。
            LastError = $"保存配置文件失败：{ex.Message}";
        }
    }
}
