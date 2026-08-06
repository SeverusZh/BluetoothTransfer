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
        var lines = SystemParameters.WheelScrollLines;
        var delta = e.Delta > 0 ? -lines : lines;
        scroll.ScrollToVerticalOffset(scroll.VerticalOffset + delta);
        e.Handled = true;
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
