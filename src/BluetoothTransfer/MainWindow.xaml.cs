using System.ComponentModel;
using System.Windows;
using Hardcodet.Wpf.TaskbarNotification;

namespace BluetoothTransfer;

public partial class MainWindow : Window
{
    private bool _forceClose;
    private TaskbarIcon? _trayIcon;

    public MainWindow()
    {
        InitializeComponent();
        var version = System.Reflection.Assembly.GetExecutingAssembly().GetName().Version;
        Title = $"蓝牙传输 v{version?.ToString(3) ?? "1.2.1"}";
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
        _trayIcon?.Dispose();
        Close();
    }
}
