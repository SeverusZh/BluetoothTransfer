using System.ComponentModel;
using System.Windows;
using Hardcodet.Wpf.TaskbarNotification;
using System.Windows.Controls;

namespace BluetoothTransfer;

public partial class MainWindow : Window
{
    private bool _forceClose;
    private TaskbarIcon? _trayIcon;

    public MainWindow()
    {
        InitializeComponent();
        AttachLogAutoscroll();
        var version = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
        Title = $"蓝牙传输 v{version?.ToString(3) ?? "1.4.0"}";
        Loaded += (_, _) =>
        {
            _trayIcon = (TaskbarIcon)FindResource("TrayIcon");
            try
            {
                using var stream = Application.GetResourceStream(
                    new Uri("pack://application:,,,/Assets/app.ico", UriKind.Absolute))?.Stream;
                if (stream != null)
                    _trayIcon.Icon = new System.Drawing.Icon(stream);
            }
            catch (Exception ex)
            {
                // 托盘图标加载失败不影响主界面，仅记录日志
                var vm = (ViewModels.OppViewModel)DataContext;
                vm.PublishLog("WARN", $"托盘图标加载失败：{ex.Message}");
            }
        };
    }

    private ScrollViewer? _logScroller;
    private bool _followLog = true;

    private void AttachLogAutoscroll()
    {
        LogListBox.Loaded += (_, _) =>
        {
            _logScroller = Behaviors.HoverScrollBehavior.FindScrollViewer(LogListBox);
            if (_logScroller == null) return;
            _logScroller.ScrollChanged += (_, e) =>
            {
                var atBottom = e.VerticalOffset >= e.ExtentHeight - e.ViewportHeight - 1;
                if (atBottom)
                    _followLog = true;
                else if (e.VerticalChange < 0)
                    _followLog = false;
            };
        };
        var vm = (ViewModels.OppViewModel)DataContext;
        vm.LogLines.CollectionChanged += (_, _) =>
        {
            if (_followLog && _logScroller != null)
                _logScroller.ScrollToEnd();
        };
    }

    private void OnDragOver(object sender, DragEventArgs e)
    {
        e.Effects = e.Data.GetDataPresent(DataFormats.FileDrop)
            ? DragDropEffects.Copy
            : DragDropEffects.None;
        e.Handled = true;
    }

    private async void OnFileDrop(object sender, DragEventArgs e)
    {
        if (!e.Data.GetDataPresent(DataFormats.FileDrop)) return;
        var files = (string[])e.Data.GetData(DataFormats.FileDrop);
        if (files.Length == 0) return;

        var vm = (ViewModels.OppViewModel)DataContext;
        await vm.SendDroppedAsync(files);
    }

    private void OnClosing(object? sender, CancelEventArgs e)
    {
        if (!_forceClose)
        {
            e.Cancel = true;
            Hide();
            _trayIcon?.ShowBalloonTip("蓝牙传输", "已最小化到系统托盘", BalloonIcon.Info);
        }
    }

    private void OnTrayDoubleClick(object sender, RoutedEventArgs e)
    {
        Show();
        WindowState = WindowState.Normal;
        Activate();
    }

    private void OnTrayShow(object sender, RoutedEventArgs e)
    {
        Show();
        WindowState = WindowState.Normal;
        Activate();
    }

    private void OnTrayExit(object sender, RoutedEventArgs e)
    {
        _forceClose = true;
        // 应用真正退出：先清理 ViewModel（退订事件、停止接收监听），再关闭窗口
        (DataContext as IDisposable)?.Dispose();
        _trayIcon?.Dispose();
        Close();
    }

}
