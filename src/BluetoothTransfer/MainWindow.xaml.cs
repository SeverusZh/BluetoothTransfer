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
        Loaded += (_, _) => _trayIcon = (TaskbarIcon)FindResource("TrayIcon");
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

        var vm = (ViewModels.MainViewModel)DataContext;
        foreach (var file in files)
        {
            if (System.IO.Directory.Exists(file))
                await vm.SendFolderAsync(file);
            else
                await vm.SendSingleFileAsync(file);
        }
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
