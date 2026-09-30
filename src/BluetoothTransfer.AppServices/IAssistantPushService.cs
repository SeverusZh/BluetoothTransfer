namespace BluetoothTransfer.Services;

/// <summary>
/// 接收助手推送服务接口：覆盖 GUI 侧实际使用的三个发送方法，
/// 签名与调用点一致（含 zip/name 可选参数与 CancellationToken）。实现见 <see cref="AssistantPushService"/>。
/// </summary>
public interface IAssistantPushService
{
    /// <summary>
    /// 发送文件。<paramref name="cancelledByPause"/> 由调用方注入“本次取消是否属于用户暂停”的判断
    /// （暂停保留 journal 与打包产物但不自动恢复；取消则清理二者）。为空时按取消处理。
    /// </summary>
    Task<bool> SendFileAsync(string deviceAddr, string filePath, bool zip = false, CancellationToken ct = default,
        Func<bool>? cancelledByPause = null);

    Task<bool> SendTextAsync(string deviceAddr, string text, string? name = null, CancellationToken ct = default,
        Func<bool>? cancelledByPause = null);

    Task<bool> SendFolderAsync(string deviceAddr, string folderPath, CancellationToken ct = default,
        Func<bool>? cancelledByPause = null);
}
