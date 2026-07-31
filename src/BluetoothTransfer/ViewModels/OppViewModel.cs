using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.ViewModels;

/// <summary>1.1 通用推送面板的设备项。</summary>
public class OppDeviceItem : INotifyPropertyChanged
{
    private string _name = "";
    private string _addr = "";
    private bool _isPaired;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string Name
    {
        get => _name;
        set => SetProperty(ref _name, value);
    }

    public string Addr
    {
        get => _addr;
        set => SetProperty(ref _addr, value);
    }

    public bool IsPaired
    {
        get => _isPaired;
        set
        {
            if (SetProperty(ref _isPaired, value))
                OnPropertyChanged(nameof(PairedDisplay));
        }
    }

    public string PairedDisplay => IsPaired ? "已配对" : "未配对";
    public string AddrDisplay => OppDiscoveryService.NormalizeAddr(Addr);

    private bool SetProperty<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value)) return false;
        field = value;
        OnPropertyChanged(name);
        return true;
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null)
        => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}

/// <summary>
/// 1.1 通用推送（OPP）面板 ViewModel：扫描/配对/发送文件/文本/文件夹，
/// 事件经 EventBus 汇集到 UI 线程。
/// </summary>
public class OppViewModel : ViewModelBase
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;
    private readonly OppPushService _push;
    private readonly Dispatcher _dispatcher;

    private bool _isScanning;
    private OppDeviceItem? _selectedDevice;
    private string _statusText = "就绪";
    private string _sendText = "";
    private double _transferProgress;
    private string _lastError = "";

    public ObservableCollection<OppDeviceItem> Devices { get; } = new();
    public ObservableCollection<string> LogLines { get; } = new();

    public string StatusText { get => _statusText; set => SetProperty(ref _statusText, value); }
    public bool IsScanning { get => _isScanning; set => SetProperty(ref _isScanning, value); }
    public OppDeviceItem? SelectedDevice
    {
        get => _selectedDevice;
        set
        {
            SetProperty(ref _selectedDevice, value);
            OnPropertyChanged(nameof(CanSend));
        }
    }
    public string SendText { get => _sendText; set => SetProperty(ref _sendText, value); }
    public double TransferProgress { get => _transferProgress; set => SetProperty(ref _transferProgress, value); }
    public bool CanSend => SelectedDevice != null && !IsScanning;
    public string LastError => _lastError;

    public RelayCommand ScanCommand { get; }
    public RelayCommand PairCommand { get; }
    public RelayCommand OpenSettingsCommand { get; }
    public RelayCommand SendTextCommand { get; }
    public RelayCommand SendFileCommand { get; }
    public RelayCommand SendFolderCommand { get; }

    public OppViewModel()
    {
        _dispatcher = Dispatcher.CurrentDispatcher;
        _events = new EventBus();
        _storage = new StorageService();
        _config = AppConfig.Load();
        _discovery = new OppDiscoveryService(_events);
        _push = new OppPushService(_events, _storage, _config, _discovery);

        ScanCommand = new RelayCommand(() => SafeAsync(ScanAsync));
        PairCommand = new RelayCommand(() => SafeAsync(PairAsync), () => SelectedDevice != null);
        OpenSettingsCommand = new RelayCommand(() => OppDiscoveryService.OpenBluetoothSettings(_events));
        SendTextCommand = new RelayCommand(() => SafeAsync(SendTextAsync), () => CanSend);
        SendFileCommand = new RelayCommand(() => SafeAsync(SendFileAsync), () => CanSend);
        SendFolderCommand = new RelayCommand(() => SafeAsync(SendFolderAsync), () => CanSend);

        _events.Subscribe<LogEvent>(OnLog);
        _events.Subscribe<TransferProgressEvent>(OnProgress);
    }

    private async Task ScanAsync()
    {
        if (IsScanning) return;
        IsScanning = true;
        try
        {
            StatusText = "正在扫描支持 OPP 的设备（5 秒）...";
            var devices = await Task.Run(() => _discovery.DiscoverAsync(seconds: 5));
            _dispatcher.Invoke(() =>
            {
                Devices.Clear();
                foreach (var d in devices)
                    Devices.Add(new OppDeviceItem { Name = d.Name, Addr = d.Addr, IsPaired = d.IsPaired });
            });
            StatusText = devices.Count > 0
                ? $"发现 {devices.Count} 个 OPP 设备"
                : "未发现 OPP 设备（确认对端已开启蓝牙并处于可发现状态，或先在系统设置中配对）";
        }
        catch (Exception ex)
        {
            _lastError = ex.Message;
            StatusText = $"扫描失败：{ex.Message}";
        }
        finally
        {
            IsScanning = false;
        }
    }

    private async Task PairAsync()
    {
        var device = SelectedDevice;
        if (device == null) return;
        StatusText = $"正在配对 {device.Name}...";
        var ok = await _discovery.PairAsync(device.Addr);
        StatusText = ok
            ? $"配对成功：{device.Name}"
            : $"配对失败：{device.Name}（可尝试在系统设置中手动配对）";
        await RefreshPairedStatusAsync();
    }

    private async Task RefreshPairedStatusAsync()
    {
        try
        {
            var devices = await Task.Run(() => _discovery.DiscoverAsync(seconds: 0));
            _dispatcher.Invoke(() =>
            {
                foreach (var item in Devices)
                {
                    var match = devices.FirstOrDefault(d => OppDiscoveryService.AddrEquals(d.Addr, item.Addr));
                    if (match != null) item.IsPaired = match.IsPaired;
                }
            });
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"刷新配对状态失败：{ex.Message}"));
        }
    }

    private async Task SendTextAsync()
    {
        var device = SelectedDevice;
        if (device == null || string.IsNullOrWhiteSpace(SendText)) return;
        StatusText = $"正在向 {device.Name} 推送文本...";
        TransferProgress = 0;
        var ok = await _push.SendTextAsync(device.Addr, SendText);
        StatusText = ok ? $"文本已推送：{device.Name}" : $"文本推送失败：{device.Name}（{_lastError}）";
        if (ok) SendText = "";
    }

    private async Task SendFileAsync()
    {
        var device = SelectedDevice;
        if (device == null) return;
        var dialog = new Microsoft.Win32.OpenFileDialog
        {
            Multiselect = true,
            Title = "选择要推送的文件"
        };
        if (dialog.ShowDialog() != true) return;

        StatusText = $"正在向 {device.Name} 推送 {dialog.FileNames.Length} 个文件...";
        var okCount = 0;
        var failedNotes = new List<string>();
        foreach (var file in dialog.FileNames)
        {
            TransferProgress = 0;
            _lastError = "";
            if (await _push.SendFileAsync(device.Addr, file))
                okCount++;
            else
                failedNotes.Add($"{System.IO.Path.GetFileName(file)}：{_lastError}");
        }
        StatusText = okCount == dialog.FileNames.Length
            ? $"已推送 {okCount} 个文件：{device.Name}"
            : $"推送完成：成功 {okCount}/{dialog.FileNames.Length}；失败：{string.Join("；", failedNotes)}";
    }

    private async Task SendFolderAsync()
    {
        var device = SelectedDevice;
        if (device == null) return;
        var dialog = new Microsoft.Win32.OpenFolderDialog
        {
            Title = "选择要推送的文件夹（将压缩为 zip）"
        };
        if (dialog.ShowDialog() != true) return;

        StatusText = $"正在压缩并向 {device.Name} 推送文件夹...";
        TransferProgress = 0;
        _lastError = "";
        var ok = await _push.SendFolderAsync(device.Addr, dialog.FolderName);
        StatusText = ok ? $"文件夹已推送：{device.Name}" : $"文件夹推送失败：{device.Name}（{_lastError}）";
    }

    private void OnLog(LogEvent e)
    {
        _dispatcher.Invoke(() =>
        {
            if (e.Level == "ERROR" || e.Level == "WARN")
                _lastError = e.Message;
            var line = $"[{DateTime.Now:HH:mm:ss}] [{e.Level}] {e.Message}";
            LogLines.Add(line);
            if (LogLines.Count > 500) LogLines.RemoveAt(0);
        });
    }

    private void OnProgress(TransferProgressEvent e)
    {
        _dispatcher.Invoke(() =>
        {
            TransferProgress = e.TotalBytes > 0 ? (double)e.BytesSent / e.TotalBytes * 100.0 : 0;
        });
    }

    private async void SafeAsync(Func<Task> action)
    {
        try
        {
            await action();
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"操作失败：{ex.Message}"));
            StatusText = $"错误：{ex.Message}";
        }
    }
}
