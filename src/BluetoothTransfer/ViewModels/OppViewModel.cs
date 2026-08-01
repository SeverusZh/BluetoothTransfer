using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.ViewModels;

/// <summary>通用推送（OPP）面板的设备项。</summary>
public class OppDeviceItem : INotifyPropertyChanged
{
    private string _name = "";
    private string _addr = "";
    private bool _isPaired;
    private bool _favorite;
    private string _alias = "";
    private string _lastConnected = "";

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

    public bool Favorite
    {
        get => _favorite;
        set => SetProperty(ref _favorite, value);
    }

    public string Alias
    {
        get => _alias;
        set
        {
            if (SetProperty(ref _alias, value))
                OnPropertyChanged(nameof(DisplayName));
        }
    }

    public string LastConnected
    {
        get => _lastConnected;
        set => SetProperty(ref _lastConnected, value);
    }

    /// <summary>展示名：别名优先，其次设备名。</summary>
    public string DisplayName => string.IsNullOrWhiteSpace(Alias) ? Name : Alias;
    public string PairedDisplay => IsPaired ? "已配对" : "未配对";
    public string FavoriteDisplay => Favorite ? "★" : "☆";
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
/// 通用推送（OPP）主界面 ViewModel：设备扫描/配对、发送（文件/文本/文件夹/剪贴板/拖放）、
/// 任务队列、传输记录、日志。事件经 EventBus 汇集到 UI 线程。
/// </summary>
public class OppViewModel : ViewModelBase
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;
    private readonly OppPushService _push;
    private readonly Dispatcher _dispatcher;
    private readonly TransferSpeedTracker _speedTracker = new();
    private readonly OppSendQueue _queue = new();

    private bool _isScanning;
    private OppDeviceItem? _selectedDevice;
    private string _statusText = "就绪";
    private string _sendText = "";
    private double _transferProgress;
    private string _progressText = "";
    private string _lastError = "";

    public ObservableCollection<OppDeviceItem> Devices { get; } = new();
    public ObservableCollection<TransferRecord> Records { get; } = new();
    public ObservableCollection<string> LogLines { get; } = new();
    public ObservableCollection<OppSendJob> Jobs => _queue.Jobs;

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
    public string ProgressText { get => _progressText; set => SetProperty(ref _progressText, value); }
    public bool CanSend => SelectedDevice != null && !IsScanning;
    public string LastError => _lastError;

    /// <summary>供代码后台等非命令路径发布日志事件（经 EventBus 汇入 UI 线程）。</summary>
    public void PublishLog(string level, string message)
        => _events.Publish(new LogEvent(level, message));

    public RelayCommand ScanCommand { get; }
    public RelayCommand PairCommand { get; }
    public RelayCommand OpenSettingsCommand { get; }
    public RelayCommand ToggleFavoriteCommand { get; }
    public RelayCommand SendTextCommand { get; }
    public RelayCommand SendFileCommand { get; }
    public RelayCommand SendFolderCommand { get; }
    public RelayCommand SendClipboardCommand { get; }
    public RelayCommand CancelAllCommand { get; }
    public RelayCommand RefreshRecordsCommand { get; }
    public RelayCommand ExportCsvCommand { get; }
    public RelayCommand ExportJsonCommand { get; }
    public RelayCommand ClearRecordsCommand { get; }

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
        ToggleFavoriteCommand = new RelayCommand(() => SafeAsync(ToggleFavoriteAsync), () => SelectedDevice != null);
        SendTextCommand = new RelayCommand(() => SafeAsync(SendTextAsync), () => CanSend);
        SendFileCommand = new RelayCommand(() => SafeAsync(SendFileAsync), () => CanSend);
        SendFolderCommand = new RelayCommand(() => SafeAsync(SendFolderAsync), () => CanSend);
        SendClipboardCommand = new RelayCommand(() => SafeAsync(SendClipboardAsync), () => CanSend);
        CancelAllCommand = new RelayCommand(() => _queue.CancelAll());
        RefreshRecordsCommand = new RelayCommand(() => SafeAsync(LoadRecordsAsync));
        ExportCsvCommand = new RelayCommand(() => ExportRecords("CSV 文件|*.csv", "csv", ExportService.ExportCsv));
        ExportJsonCommand = new RelayCommand(() => ExportRecords("JSON 文件|*.json", "json", ExportService.ExportJson));
        ClearRecordsCommand = new RelayCommand(ClearRecords);

        _events.Subscribe<LogEvent>(OnLog);
        _events.Subscribe<TransferProgressEvent>(OnProgress);
        _ = LoadRecordsAsync();
    }

    // ------------------------------------------------------------------ 设备

    private async Task ScanAsync()
    {
        if (IsScanning) return;
        IsScanning = true;
        try
        {
            StatusText = "正在扫描支持 OPP 的设备（5 秒）...";
            var devices = await Task.Run(() => _discovery.DiscoverAsync(seconds: 5));
            var saved = _storage.GetDevices();
            _dispatcher.Invoke(() =>
            {
                Devices.Clear();
                foreach (var d in devices.OrderByDescending(x => x.IsPaired))
                {
                    var info = saved.FirstOrDefault(s => OppDiscoveryService.AddrEquals(s.Addr, d.Addr));
                    Devices.Add(new OppDeviceItem
                    {
                        Name = d.Name,
                        Addr = d.Addr,
                        IsPaired = d.IsPaired,
                        Favorite = info?.Favorite ?? false,
                        Alias = info?.Alias ?? "",
                        LastConnected = info?.LastConnected ?? ""
                    });
                }
            });
            StatusText = Devices.Count > 0
                ? $"发现 {Devices.Count} 个 OPP 设备"
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
        StatusText = $"正在配对 {device.DisplayName}...";
        var ok = await _discovery.PairAsync(device.Addr);
        StatusText = ok
            ? $"配对成功：{device.DisplayName}"
            : $"配对失败：{device.DisplayName}（可尝试在系统设置中手动配对）";
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

    private async Task ToggleFavoriteAsync()
    {
        var device = SelectedDevice;
        if (device == null) return;
        var next = !device.Favorite;
        await Task.Run(() => _storage.SetDeviceFavorite(device.Addr, next));
        device.Favorite = next;
        StatusText = next ? $"已收藏：{device.DisplayName}" : $"已取消收藏：{device.DisplayName}";
    }

    // ------------------------------------------------------------------ 发送

    private void EnqueueAndStart(IEnumerable<OppSendJob> jobs)
    {
        _queue.Enqueue(jobs);
        _speedTracker.Reset();
        ProgressText = "";
        TransferProgress = 0;
        if (_queue.IsRunning) return;
        var device = SelectedDevice;
        if (device == null) return;
        _ = Task.Run(() => _queue.StartAsync(device.Addr, async (job, ct) =>
        {
            try
            {
                return job.Kind switch
                {
                    "folder" => await _push.SendFolderAsync(device.Addr, job.SourcePath, ct),
                    "text" => await _push.SendTextAsync(device.Addr, job.SourcePath, job.DisplayName, ct),
                    _ => await _push.SendFileAsync(device.Addr, job.SourcePath, zip: false, ct)
                };
            }
            finally
            {
                _ = LoadRecordsAsync();
            }
        }));
    }

    private async Task SendTextAsync()
    {
        var device = SelectedDevice;
        if (device == null || string.IsNullOrWhiteSpace(SendText)) return;
        var text = SendText;
        StatusText = $"正在向 {device.DisplayName} 推送文本...";
        TransferProgress = 0;
        var ok = await _push.SendTextAsync(device.Addr, text);
        StatusText = ok ? $"文本已推送：{device.DisplayName}" : $"文本推送失败：{device.DisplayName}（{_lastError}）";
        if (ok) SendText = "";
        await LoadRecordsAsync();
    }

    private Task SendFileAsync()
    {
        var device = SelectedDevice;
        if (device == null) return Task.CompletedTask;
        var dialog = new Microsoft.Win32.OpenFileDialog
        {
            Multiselect = true,
            Title = "选择要推送的文件"
        };
        if (dialog.ShowDialog() != true) return Task.CompletedTask;

        EnqueueAndStart(dialog.FileNames.Select(f => new OppSendJob
        {
            Kind = "file",
            SourcePath = f,
            DisplayName = System.IO.Path.GetFileName(f),
            Size = new System.IO.FileInfo(f).Length
        }));
        StatusText = $"已加入队列 {dialog.FileNames.Length} 个文件：{device.DisplayName}";
        return Task.CompletedTask;
    }

    private Task SendFolderAsync()
    {
        var device = SelectedDevice;
        if (device == null) return Task.CompletedTask;
        var dialog = new Microsoft.Win32.OpenFolderDialog
        {
            Title = "选择要推送的文件夹（将压缩为 zip）"
        };
        if (dialog.ShowDialog() != true) return Task.CompletedTask;

        EnqueueAndStart(new[]
        {
            new OppSendJob
            {
                Kind = "folder",
                SourcePath = dialog.FolderName,
                DisplayName = System.IO.Path.GetFileName(dialog.FolderName.TrimEnd('\\', '/')),
                Size = 0
            }
        });
        StatusText = $"已加入队列：文件夹 {dialog.FolderName}";
        return Task.CompletedTask;
    }

    private Task SendClipboardAsync()
    {
        var device = SelectedDevice;
        if (device == null) return Task.CompletedTask;
        try
        {
            if (Clipboard.ContainsText())
            {
                var text = Clipboard.GetText();
                EnqueueAndStart(new[]
                {
                    new OppSendJob
                    {
                        Kind = "text",
                        SourcePath = text,
                        DisplayName = _config.PushTextFileName,
                        Size = System.Text.Encoding.UTF8.GetByteCount(text)
                    }
                });
                StatusText = $"已加入队列：剪贴板文本（{text.Length} 字符）";
                return Task.CompletedTask;
            }
            if (Clipboard.ContainsImage())
            {
                var source = Clipboard.GetImage();
                if (source == null) return Task.CompletedTask;
                var encoder = new PngBitmapEncoder();
                encoder.Frames.Add(BitmapFrame.Create(source));
                var tempPath = System.IO.Path.Combine(System.IO.Path.GetTempPath(), $"bt_clipboard_{Guid.NewGuid():N}.png");
                using (var fs = new System.IO.FileStream(tempPath, System.IO.FileMode.Create))
                    encoder.Save(fs);
                EnqueueAndStart(new[]
                {
                    new OppSendJob
                    {
                        Kind = "file",
                        SourcePath = tempPath,
                        DisplayName = "clipboard.png",
                        Size = new System.IO.FileInfo(tempPath).Length
                    }
                });
                StatusText = "已加入队列：剪贴板图片（clipboard.png）";
                return Task.CompletedTask;
            }
            StatusText = "剪贴板中没有可发送的文本或图片";
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"读取剪贴板失败：{ex.Message}"));
        }
        return Task.CompletedTask;
    }

    /// <summary>拖放入口：文件/文件夹混合加入队列。</summary>
    public Task SendDroppedAsync(IEnumerable<string> paths)
    {
        var device = SelectedDevice;
        if (device == null)
        {
            StatusText = "请先在设备列表中选中目标设备";
            return Task.CompletedTask;
        }
        var jobs = paths.Select(p =>
            System.IO.Directory.Exists(p)
                ? new OppSendJob
                {
                    Kind = "folder",
                    SourcePath = p,
                    DisplayName = System.IO.Path.GetFileName(p.TrimEnd('\\', '/')),
                    Size = 0
                }
                : new OppSendJob
                {
                    Kind = "file",
                    SourcePath = p,
                    DisplayName = System.IO.Path.GetFileName(p),
                    Size = System.IO.File.Exists(p) ? new System.IO.FileInfo(p).Length : 0
                }).ToList();
        EnqueueAndStart(jobs);
        StatusText = $"已加入队列 {jobs.Count} 项：{device.DisplayName}";
        return Task.CompletedTask;
    }

    // ------------------------------------------------------------------ 记录

    private async Task LoadRecordsAsync()
    {
        try
        {
            var records = await Task.Run(() => _storage.GetRecords(limit: 200));
            _dispatcher.Invoke(() =>
            {
                Records.Clear();
                foreach (var r in records) Records.Add(r);
            });
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"加载记录失败：{ex.Message}"));
        }
    }

    private void ExportRecords(string filter, string ext, Func<List<TransferRecord>, string, string> export)
    {
        try
        {
            var dialog = new Microsoft.Win32.SaveFileDialog
            {
                Filter = filter,
                FileName = $"transfer_records_{DateTime.Now:yyyyMMdd_HHmmss}.{ext}"
            };
            if (dialog.ShowDialog() != true) return;
            var records = _storage.GetRecords(limit: 10000);
            export(records, dialog.FileName);
            StatusText = $"已导出 {records.Count} 条记录到 {ext.ToUpperInvariant()}";
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"导出失败：{ex.Message}"));
            StatusText = $"导出失败：{ex.Message}";
        }
    }

    private void ClearRecords()
    {
        var total = _storage.GetStats().totalCount;
        if (total == 0)
        {
            StatusText = "没有可清空的记录";
            return;
        }
        var confirm = MessageBox.Show(
            $"确定要清空全部 {total} 条传输记录吗？该操作不可恢复。",
            "清空传输记录",
            MessageBoxButton.YesNo,
            MessageBoxImage.Warning);
        if (confirm != MessageBoxResult.Yes) return;

        try
        {
            var deleted = _storage.ClearRecords();
            Records.Clear();
            StatusText = $"已清空 {deleted} 条传输记录";
        }
        catch (Exception ex)
        {
            _events.Publish(new LogEvent("ERROR", $"清空传输记录失败：{ex.Message}"));
            StatusText = $"清空失败：{ex.Message}";
        }
    }

    // ------------------------------------------------------------------ 事件

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
            _speedTracker.AddSample(e.BytesSent);
            var speed = _speedTracker.SpeedBytesPerSecond;
            var eta = _speedTracker.Eta(e.TotalBytes);
            ProgressText = speed > 0
                ? $"{speed / 1024.0:F1} KB/s{(eta.HasValue ? $" · 剩余 {eta.Value.TotalSeconds:0}s" : "")}"
                : "";
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
