namespace BluetoothTransfer.Services;

/// <summary>
/// 接收助手推送服务接口：覆盖 GUI 侧实际使用的三个发送方法，
/// 签名与调用点一致（含 zip/name 可选参数与 CancellationToken）。实现见 <see cref="AssistantPushService"/>。
/// </summary>
public interface IAssistantPushService
{
    Task<bool> SendFileAsync(string deviceAddr, string filePath, bool zip = false, CancellationToken ct = default);

    Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default);

    Task<bool> SendFolderAsync(string deviceAddr, string folderPath, CancellationToken ct = default);
}
