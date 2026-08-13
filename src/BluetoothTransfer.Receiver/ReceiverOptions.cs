using System.IO;

namespace BluetoothTransfer.Receiver;

public sealed class ReceiverOptions
{
    public string SaveDir { get; init; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        "Downloads", "BluetoothReceive");

    /// <summary>true 时每个传输请求在控制台询问 y/N（默认开启，防止已配对设备直接落盘）。</summary>
    public bool Ask { get; init; } = true;
}
