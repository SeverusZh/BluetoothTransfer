using System.Collections.Specialized;
using System.IO;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows;
using System.Windows.Media.Imaging;

namespace BluetoothTransfer.Services;

/// <summary>
/// 后台线程准备好的剪贴板数据：文本 / 已冻结的图片位图 / 文件路径。
/// 位图在后台线程解码并 Freeze 后，才能安全地交给 UI 线程写入剪贴板。
/// </summary>
internal sealed class WpfClipboardPayload
{
    public TransferCopyKind Kind { get; init; }

    public string Text { get; init; } = "";

    public BitmapSource? Image { get; init; }

    public string Path { get; init; } = "";
}

/// <summary>
/// 剪贴板写入适配器（薄 WPF 层，剪贴板与图片解码依赖 WPF，故不放纯逻辑层）：
/// 1. <see cref="Prepare"/> 在后台线程执行相对重的数据准备（图片解码），文本/文件仅做轻量包装；
/// 2. <see cref="TryWrite"/> 必须在 UI（STA）线程调用，负责把数据放进 Windows 剪贴板；
///    对「剪贴板被其它程序占用」做有限次退避重试，失败统一返回中文原因，由 ViewModel 上报状态栏与日志（不静默）。
/// </summary>
internal static class WpfClipboardWriter
{
    /// <summary>
    /// 剪贴板占用时的最多尝试次数与退避间隔：剪贴板是系统全局资源，其它程序短暂占用时会写入失败，
    /// 这里做有限次短间隔重试，避免长时间阻塞 UI 线程；仍失败则立即上报中文错误。
    /// </summary>
    private const int RetryTimes = 3;
    private const int RetryDelayMs = 80;

    /// <summary>
    /// 后台准备剪贴板数据：图片解码为冻结位图；文本/文件无需额外处理。
    /// 返回的 Payload 为 null 时 Error 为可直接展示的中文原因。
    /// </summary>
    public static (WpfClipboardPayload? Payload, string Error) Prepare(TransferCopyPlan plan)
    {
        ArgumentNullException.ThrowIfNull(plan);
        switch (plan.Kind)
        {
            case TransferCopyKind.Text:
                return (new WpfClipboardPayload { Kind = plan.Kind, Text = plan.Payload }, "");
            case TransferCopyKind.File:
                return (new WpfClipboardPayload { Kind = plan.Kind, Path = plan.Payload }, "");
            case TransferCopyKind.Image:
                try
                {
                    return (new WpfClipboardPayload
                    {
                        Kind = plan.Kind,
                        Path = plan.Payload,
                        Image = LoadImage(plan.Payload)
                    }, "");
                }
                catch (Exception ex)
                {
                    // 可能是不受系统支持的图片格式（如缺少编解码器）或文件损坏：不静默，交由状态栏提示。
                    return (null, $"读取图片内容失败（可能是不受支持的图片格式）：{ex.Message}");
                }
            default:
                return (null, "复制失败：剪贴板目标类型未知");
        }
    }

    /// <summary>
    /// 在 UI（STA）线程把准备好的数据写入剪贴板。返回 false 时 error 为可直接展示的中文原因。
    /// </summary>
    public static bool TryWrite(WpfClipboardPayload payload, out string error)
    {
        ArgumentNullException.ThrowIfNull(payload);
        for (var attempt = 1; ; attempt++)
        {
            try
            {
                WriteOnce(payload);
                error = "";
                return true;
            }
            catch (ExternalException) when (attempt < RetryTimes)
            {
                // 典型为 CLIPBRD_E_CANT_OPEN：剪贴板被其它程序短暂占用，退避后重试。
                Thread.Sleep(RetryDelayMs * attempt);
            }
            catch (ExternalException ex)
            {
                error = $"写入剪贴板失败（剪贴板可能被其它程序占用，请稍后重试）：{ex.Message}";
                return false;
            }
            catch (Exception ex)
            {
                error = $"写入剪贴板失败：{ex.Message}";
                return false;
            }
        }
    }

    /// <summary>按目标类型执行一次剪贴板写入（必须在 UI/STA 线程调用）。</summary>
    private static void WriteOnce(WpfClipboardPayload payload)
    {
        switch (payload.Kind)
        {
            case TransferCopyKind.Text:
                Clipboard.SetText(payload.Text);
                break;
            case TransferCopyKind.Image:
                var image = payload.Image;
                if (image == null) throw new InvalidOperationException("图片数据为空");
                Clipboard.SetImage(image);
                break;
            case TransferCopyKind.File:
                Clipboard.SetFileDropList(new StringCollection { payload.Path });
                break;
            default:
                throw new InvalidOperationException("未知的剪贴板目标类型");
        }
    }

    /// <summary>
    /// 解码本地图片为冻结位图：可在后台线程创建，Freeze 后可跨线程交给 UI 线程写入剪贴板。
    /// 大小上限由 <see cref="TransferCopyPlanner.MaxImageBytes"/> 在规划阶段拦截，这里再做一次防御。
    /// </summary>
    private static BitmapSource LoadImage(string path)
    {
        var info = new FileInfo(path);
        if (!info.Exists) throw new FileNotFoundException("图片文件不存在", path);
        if (info.Length > TransferCopyPlanner.MaxImageBytes)
            throw new InvalidOperationException($"图片文件过大（{info.Length / (1024.0 * 1024):0.##} MB），超过复制上限");

        using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.Read);
        var image = new BitmapImage();
        image.BeginInit();
        image.CacheOption = BitmapCacheOption.OnLoad;
        image.CreateOptions = BitmapCreateOptions.IgnoreImageCache;
        image.StreamSource = stream;
        image.EndInit();
        image.Freeze();
        return image;
    }
}
