using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;

namespace BluetoothTransfer.Behaviors;

/// <summary>
/// 悬停即滚：鼠标悬停在列表上时滚轮滚动其内部 ScrollViewer，无需先聚焦。
/// WPF 默认 MouseWheel 只作用于已聚焦控件，本行为通过 PreviewMouseWheel 修复。
/// </summary>
public static class HoverScrollBehavior
{
    public static readonly DependencyProperty EnabledProperty =
        DependencyProperty.RegisterAttached(
            "Enabled", typeof(bool), typeof(HoverScrollBehavior),
            new PropertyMetadata(false, OnEnabledChanged));

    public static void SetEnabled(DependencyObject obj, bool value) => obj.SetValue(EnabledProperty, value);
    public static bool GetEnabled(DependencyObject obj) => (bool)obj.GetValue(EnabledProperty);

    private static void OnEnabledChanged(DependencyObject d, DependencyPropertyChangedEventArgs e)
    {
        if (d is not UIElement element) return;
        if ((bool)e.NewValue) element.PreviewMouseWheel += OnPreviewMouseWheel;
        else element.PreviewMouseWheel -= OnPreviewMouseWheel;
    }

    private static void OnPreviewMouseWheel(object sender, MouseWheelEventArgs e)
    {
        if (sender is not DependencyObject root) return;
        var scroll = FindScrollViewer(root);
        if (scroll == null) return;

        // 边界交还策略：列表本身不可滚动（ScrollableHeight<=0），或已到顶部仍向上滚、
        // 已到底部仍向下滚时，不拦截滚轮事件，将其交还给外层 ScrollViewer（主区滚动容器），
        // 避免鼠标停在列表上时外层完全收不到滚轮、无法继续滚动页面。
        // 容差用于抵消 DPI 缩放/布局取整造成的 VerticalOffset 与 ScrollableHeight 的浮点误差。
        const double boundaryEpsilon = 0.5;
        var atTop = scroll.VerticalOffset <= boundaryEpsilon;
        var atBottom = scroll.VerticalOffset >= scroll.ScrollableHeight - boundaryEpsilon;
        var handBack = scroll.ScrollableHeight <= 0
            || (e.Delta > 0 && atTop)
            || (e.Delta < 0 && atBottom);

        if (handBack)
        {
            HandOffToAncestorScrollViewer(root, e);
            return;
        }

        // 列表内部仍可滚动：维持原有行为，手动滚动内部 ScrollViewer 并消费事件。
        var lines = SystemParameters.WheelScrollLines;
        var delta = e.Delta > 0 ? -lines : lines;
        scroll.ScrollToVerticalOffset(scroll.VerticalOffset + delta);
        e.Handled = true;
    }

    /// <summary>
    /// 把滚轮交还给外层 ScrollViewer（主区滚动容器）。
    /// 说明：不能只“不设置 Handled 直接 return”——WPF 内部 ScrollViewer.OnMouseWheel
    /// 到达边界时仍会无条件 Handled=true，事件无法自然冒泡到外层；net8 WPF 的
    /// HandlesMouseWheelScrolling 又是 internal，无法关闭其默认滚轮处理。因此这里在外层
    /// 尚存时主动转发一次等价滚轮事件，并消费原事件，让外层按自身规则滚动。
    /// 没有外层滚动容器时保持原样（不处理、不消费）。
    /// </summary>
    private static void HandOffToAncestorScrollViewer(DependencyObject root, MouseWheelEventArgs e)
    {
        var outer = FindAncestorScrollViewer(root);
        if (outer == null) return;

        var forwarded = new MouseWheelEventArgs(e.MouseDevice, e.Timestamp, e.Delta)
        {
            RoutedEvent = UIElement.MouseWheelEvent,
            Source = outer,
        };
        outer.RaiseEvent(forwarded);
        e.Handled = true;
    }

    /// <summary>沿可视树向上查找最近的 ScrollViewer（即列表外层的主区滚动容器）。</summary>
    private static ScrollViewer? FindAncestorScrollViewer(DependencyObject node)
    {
        var current = VisualTreeHelper.GetParent(node);
        while (current != null)
        {
            if (current is ScrollViewer sv) return sv;
            current = VisualTreeHelper.GetParent(current);
        }
        return null;
    }

    public static ScrollViewer? FindScrollViewer(DependencyObject root)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is ScrollViewer sv) return sv;
            var nested = FindScrollViewer(child);
            if (nested != null) return nested;
        }
        return null;
    }
}
