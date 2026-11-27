using BluetoothTransfer.Core.Protocol;

namespace BluetoothTransfer.Services;

/// <summary>
/// 内置接收助手服务接口：覆盖 GUI 侧实际使用的监听启停、IsListening、
/// AskHandler 属性与三个事件。实现见 <see cref="ReceiveService"/>。
/// </summary>
public interface IReceiveService
{
    bool IsListening { get; }

    /// <summary>询问处理器：由 UI 弹出确认；返回 true 接受。</summary>
    Func<AsstHello, Task<bool>>? AskHandler { get; set; }

    event Action<string, long, long>? ProgressChanged;
    event Action<string>? Completed;
    event Action<string, string>? Logged;

    Task StartAsync(string saveDir, bool ask, CancellationToken ct = default);

    Task StopAsync();
}
