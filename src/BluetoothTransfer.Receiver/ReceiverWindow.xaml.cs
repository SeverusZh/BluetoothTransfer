using System.Collections.ObjectModel;
using System.Windows;
using System.Windows.Threading;
using BluetoothTransfer.Core.Protocol;
using BluetoothTransfer.Core.Server;

namespace BluetoothTransfer.Receiver;

public partial class ReceiverWindow : Window
{
    private readonly Dispatcher _dispatcher;
    private readonly CancellationTokenSource _cts = new();
    private AsstRfcommService? _listener;
    private AssistantServer? _server;
    private bool _ask;
    private string _saveDir;
    private string _currentName = "";

    public ObservableCollection<string> Completed { get; } = new();

    public ReceiverWindow(string saveDir, bool ask)
    {
        InitializeComponent();
        _dispatcher = Dispatcher;
        _saveDir = saveDir;
        _ask = ask;
        CompletedList.ItemsSource = Completed;
        SaveDirBox.Text = saveDir;
        SaveDirText.Text = $"接收目录：{saveDir}";
        AskCheck.IsChecked = ask;
        Closed += OnWindowClosed;
    }

    private async void OnLoaded(object sender, RoutedEventArgs e)
    {
        try
        {
            _listener = await AsstRfcommService.StartAsync();
            var sink = new AsstFileSink(_saveDir);
            _server = new AssistantServer(_listener, sink,
                approval: AskAsync,
                onLog: (level, msg) => _dispatcher.Invoke(() => StatusText.Text = msg),
                onProgress: (hello, sent, total) => _dispatcher.Invoke(() =>
                {
                    _currentName = hello.FileName;
                    CurrentName.Text = hello.FileName;
                    Progress.Value = total > 0 ? sent * 100.0 / total : 0;
                    ProgressText.Text = $"{sent:N0} / {total:N0} 字节";
                }),
                onCompleted: (hello, _) => _dispatcher.Invoke(() =>
                {
                    Completed.Add($"{DateTime.Now:HH:mm:ss} {hello.FileName}");
                    if (string.Equals(_currentName, hello.FileName, StringComparison.OrdinalIgnoreCase))
                    {
                        CurrentName.Text = "（无）";
                        Progress.Value = 0;
                        ProgressText.Text = "";
                    }
                }));
            StatusText.Text = "监听中…";
            await _server.RunAsync(_cts.Token);
        }
        catch (Exception ex)
        {
            StatusText.Text = $"启动失败：{ex.Message}";
        }
    }

    private async Task<AsstOfferStatus> AskAsync(AsstHello hello)
    {
        if (!_ask) return AsstOfferStatus.Accept;
        var result = await _dispatcher.InvokeAsync(() =>
            MessageBox.Show(this, $"接收文件 {hello.FileName}（{hello.FileSize:N0} 字节）？",
                "蓝牙接收助手", MessageBoxButton.YesNo, MessageBoxImage.Question));
        return result == MessageBoxResult.Yes ? AsstOfferStatus.Accept : AsstOfferStatus.Reject;
    }

    private void OnBrowse(object sender, RoutedEventArgs e)
    {
        var dialog = new Microsoft.Win32.OpenFolderDialog { Title = "选择接收目录" };
        if (dialog.ShowDialog(this) == true)
        {
            _saveDir = dialog.FolderName;
            SaveDirBox.Text = _saveDir;
            SaveDirText.Text = $"接收目录：{_saveDir}（重启后生效）";
        }
    }

    private void OnAskChanged(object sender, RoutedEventArgs e) => _ask = AskCheck.IsChecked == true;

    private void OnWindowClosed(object? sender, EventArgs e)
    {
        _cts.Cancel();
        _server?.DisposeAsync().AsTask().GetAwaiter().GetResult();
        _listener?.DisposeAsync().AsTask().GetAwaiter().GetResult();
    }
}
