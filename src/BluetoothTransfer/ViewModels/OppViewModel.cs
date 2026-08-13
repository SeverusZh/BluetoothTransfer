using System.Collections.ObjectModel;
using System.ComponentModel;
using System.Runtime.CompilerServices;
using System.Windows;
using System.Windows.Media.Imaging;
using System.Windows.Threading;
using BluetoothTransfer.Core.Discovery;
using BluetoothTransfer.Core.Protocol;
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
    private bool _isAssistant;

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
        set
        {
            if (SetProperty(ref _favorite, value))
                OnPropertyChanged(nameof(FavoriteDisplay));
        }
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

    public bool IsAssistant
    {
        get => _isAssistant;
        set => SetProperty(ref _isAssistant, value);
    }

    /// <summary>WinRT 设备条目 Id（用于 SDP 探测助手服务）。</summary>
    public string DeviceId { get; set; } = "";

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
/// 实现 IDisposable：退出时退订事件并停止接收监听，避免监听进程退出时才被释放。
/// </summary>
public class OppViewModel : ViewModelBase, IDisposable
{
    private readonly EventBus _events;
    private readonly StorageService _storage;
    private readonly AppConfig _config;
    private readonly OppDiscoveryService _discovery;
    private readonly OppPushService _push;
    private readonly AssistantPushService _assistantPush;
    private readonly ReceiveService _receive;
    // EventBus/接收服务的事件 handler 引用：保存引用以便 Dispose 时退订（-=/Unsubscribe 必须引用同一委托实例）
    private readonly Action<LogEvent> _logHandler;
    private readonly Action<TransferProgressEvent> _progressHandler;
    private readonly Dictionary<string, bool> _assistantCache = new(StringComparer.OrdinalIgnoreCase);
    private bool? _queueUseAssistant;
    private readonly Dispatcher _dispatcher;
    private readonly TransferSpeedTracker _speedTracker = new();
    private readonly OppSendQueue _queue = new();
    // 组合式拆分：设备域与记录域的纯内部逻辑分别下沉到 DevicePaneController / RecordsPaneController，
    // 本 VM 保持对外门面，公开 API 不变。`_assistantCache` 与发送队列（ResolveAssistantOnceAsync）共用，
    // 因此把它传给 DevicePaneController 同一实例引用，保证缓存写入语义跨两域一致。
    private readonly DevicePaneController _devicePane;
    private readonly RecordsPaneController _recordsPane;

    private bool _isScanning;
    private OppDeviceItem? _selectedDevice;
    private string _statusText = "就绪";
    private string _sendText = "";
    private double _transferProgress;
    private string _progressText = "";
    private string _lastError = "";
    private OppDeviceItem? _queueDevice;

    public ObservableCollection<OppDeviceItem> Devices { get; } = new();
    public ObservableCollection<TransferRecord> Records { get; } = new();
    public ObservableCollection<string> LogLines { get; } = new();
    public ObservableCollection<OppSendJob> Jobs => _queue.Jobs;

    public string StatusText { get => _statusText; set => SetProperty(ref _statusText, value); }

    /// <summary>版本号单一事实源：取程序集版本（csproj &lt;Version&gt;）的 major.minor.build 拼接显示。</summary>
    public string VersionText =>
        "v" + (System.Reflection.Assembly.GetExecutingAssembly().GetName().Version?.ToString(3) ?? "");

    public bool IsScanning
    {
        get => _isScanning;
        set
        {
            if (!SetProperty(ref _isScanning, value)) return;
            // 扫描状态影响 CanSend，主动刷新发送类命令的使能（避免扫描完成后按钮仍灰着）
            SendTextCommand.RaiseCanExecuteChanged();
            SendFileCommand.RaiseCanExecuteChanged();
            SendFolderCommand.RaiseCanExecuteChanged();
            SendClipboardCommand.RaiseCanExecuteChanged();
        }
    }
    public OppDeviceItem? SelectedDevice
    {
        get => _selectedDevice;
        set
        {
            if (!SetProperty(ref _selectedDevice, value)) return;
            OnPropertyChanged(nameof(CanSend));
            // 选中设备影响 CanSend 及相关命令使能，主动刷新（不依赖 CommandManager 的输入事件触发）
            SendTextCommand.RaiseCanExecuteChanged();
            SendFileCommand.RaiseCanExecuteChanged();
            SendFolderCommand.RaiseCanExecuteChanged();
            SendClipboardCommand.RaiseCanExecuteChanged();
            PairCommand.RaiseCanExecuteChanged();
            ToggleFavoriteCommand.RaiseCanExecuteChanged();
        }
    }
    public string SendText { get => _sendText; set => SetProperty(ref _sendText, value); }
    public double TransferProgress { get => _transferProgress; set => SetProperty(ref _transferProgress, value); }
    public string ProgressText { get => _progressText; set => SetProperty(ref _progressText, value); }
    public bool CanSend => SelectedDevice != null && !IsScanning;
    public string LastError => _lastError;

    /// <summary>发送通道模式：auto（探测助手，否则 OPP）/ assistant / opp。</summary>
    public string SelectedTransferMode
    {
        get => _config.TransferMode;
        set
        {
            if (_config.TransferMode == value) return;
            _config.TransferMode = value;
            _config.Save();
            // 保存失败不再静默：经日志上报（_events 仅在构造后可达，无空引用风险）
            if (_config.LastError is { } saveErr)
                PublishLog(TransferConst.LogWarn, saveErr);
            OnPropertyChanged();
        }
    }

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
    public RelayCommand PauseJobCommand { get; }
    public RelayCommand ContinueJobCommand { get; }
    public RelayCommand RemoveJobCommand { get; }
    public RelayCommand ToggleReceiveCommand { get; }
    public RelayCommand BrowseReceiveDirCommand { get; }

    /// <summary>接收助手：是否监听中。</summary>
    public bool IsReceiving => _receive.IsListening;
    public string ReceiveToggleText => _receive.IsListening ? "停止监听" : "开始监听";
    public string ReceiveSaveDir
    {
        get => _config.ReceiveDirectory;
        set
        {
            if (_config.ReceiveDirectory == value) return;
            _config.ReceiveDirectory = value;
            _config.Save();
            // 保存失败不再静默：经日志上报
            if (_config.LastError is { } saveErr)
                PublishLog(TransferConst.LogWarn, saveErr);
            OnPropertyChanged();
        }
    }
    public bool ReceiveAsk
    {
        get => _config.ReceiveAsk;
        set
        {
            if (_config.ReceiveAsk == value) return;
            _config.ReceiveAsk = value;
            _config.Save();
            // 保存失败不再静默：经日志上报
            if (_config.LastError is { } saveErr)
                PublishLog(TransferConst.LogWarn, saveErr);
            OnPropertyChanged();
        }
    }
    private string _receiveCurrentFile = "";
    public string ReceiveCurrentFile { get => _receiveCurrentFile; set => SetProperty(ref _receiveCurrentFile, value); }
    private double _receiveProgress;
    public double ReceiveProgress { get => _receiveProgress; set => SetProperty(ref _receiveProgress, value); }
    private string _receiveProgressText = "";
    public string ReceiveProgressText { get => _receiveProgressText; set => SetProperty(ref _receiveProgressText, value); }
    public ObservableCollection<string> ReceiveCompleted { get; } = new();

    public OppViewModel()
    {
        _dispatcher = Dispatcher.CurrentDispatcher;
        _events = new EventBus();
        _storage = new StorageService();
        _config = AppConfig.Load();
        // 读取配置失败时经日志上报（_events 已在上面初始化，可安全发布）
        if (_config.LastError is { } loadErr)
            _events.Publish(new LogEvent(TransferConst.LogWarn, loadErr));
        _discovery = new OppDiscoveryService(_events);
        _push = new OppPushService(_events, _storage, _config, _discovery);
        _assistantPush = new AssistantPushService(_events, _storage, _config, _discovery);
        _receive = new ReceiveService(_storage, _events)
        {
            AskHandler = AskReceiveAsync
        };
        _receive.ProgressChanged += OnReceiveProgress;
        _receive.Completed += OnReceiveCompleted;
        _receive.Logged += OnReceiveLogged;

        // 设备域 / 记录域控制器：注入服务、调度器与 VM 状态回写回调（见对应组件类说明）。
        _devicePane = new DevicePaneController(
            _discovery, _storage, _dispatcher, Devices, _assistantCache,
            () => IsScanning, v => IsScanning = v,
            v => StatusText = v, v => _lastError = v,
            (lvl, msg) => _events.Publish(new LogEvent(lvl, msg)));
        _recordsPane = new RecordsPaneController(
            _storage, _dispatcher, Records,
            v => StatusText = v,
            (lvl, msg) => _events.Publish(new LogEvent(lvl, msg)));

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
        ExportCsvCommand = new RelayCommand(() => SafeAsync(() =>
            ExportRecordsAsync("CSV 文件|*.csv", "csv", ExportService.ExportCsv)));
        ExportJsonCommand = new RelayCommand(() => SafeAsync(() =>
            ExportRecordsAsync("JSON 文件|*.json", "json", ExportService.ExportJson)));
        ClearRecordsCommand = new RelayCommand(() => SafeAsync(ClearRecordsAsync));
        PauseJobCommand = new RelayCommand(job => _queue.Pause((OppSendJob)job!));
        ContinueJobCommand = new RelayCommand(job =>
        {
            var target = (OppSendJob)job!;
            _queue.Continue(target);
            var device = _queueDevice ?? SelectedDevice;
            if (device != null && target.Status == OppJobStatus.Pending)
                SafeAsync(() => StartQueueIfNeeded(device));
        });
        RemoveJobCommand = new RelayCommand(job => _queue.Remove((OppSendJob)job!));
        ToggleReceiveCommand = new RelayCommand(() => SafeAsync(ToggleReceiveAsync));
        BrowseReceiveDirCommand = new RelayCommand(BrowseReceiveDir);

        _events.Subscribe(_logHandler = new Action<LogEvent>(OnLog));
        _events.Subscribe(_progressHandler = new Action<TransferProgressEvent>(OnProgress));
        _ = LoadRecordsAsync();
        _ = _devicePane.LoadFavoriteDevicesAsync();
    }

    // ------------------------------------------------------------------ 设备
    // 设备域的纯内部逻辑（扫描/配对/收藏/助手探测与合并）已下沉到 DevicePaneController，
    // 以下方法仅作门面委托，保持命令与 SelectedDevice/IsScanning/StatusText/LastError 语义不变。

    private Task ScanAsync() => _devicePane.ScanAsync();

    private async Task PairAsync()
    {
        var device = SelectedDevice;
        await _devicePane.PairAsync(device);
    }

    private async Task ToggleFavoriteAsync()
    {
        var device = SelectedDevice;
        await _devicePane.ToggleFavoriteAsync(device);
    }

    // ------------------------------------------------------------------ 接收助手

    private async Task ToggleReceiveAsync()
    {
        if (_receive.IsListening)
        {
            await _receive.StopAsync();
        }
        else
        {
            await _receive.StartAsync(ReceiveSaveDir, ReceiveAsk);
        }
        OnPropertyChanged(nameof(IsReceiving));
        OnPropertyChanged(nameof(ReceiveToggleText));
    }

    private void BrowseReceiveDir()
    {
        var dialog = new Microsoft.Win32.OpenFolderDialog { Title = "选择接收保存目录" };
        if (dialog.ShowDialog() == true)
            ReceiveSaveDir = dialog.FolderName;
    }

    private async Task<bool> AskReceiveAsync(AsstHello hello)
    {
        var result = await _dispatcher.InvokeAsync(() =>
            MessageBox.Show($"接收文件 {hello.FileName}（{hello.FileSize:N0} 字节）？",
                "蓝牙传输", MessageBoxButton.YesNo, MessageBoxImage.Question));
        return result == MessageBoxResult.Yes;
    }

    // 下列接收事件订阅为具名方法：Dispose 可经 -= 精确退订同一委托实例（lambda 捕获 this 无法退订）。

    private void OnReceiveProgress(string name, long sent, long total)
    {
        _dispatcher.Invoke(() =>
        {
            ReceiveCurrentFile = name;
            ReceiveProgress = total > 0 ? sent * 100.0 / total : 0;
            ReceiveProgressText = $"{sent:N0} / {total:N0} 字节";
        });
    }

    private void OnReceiveCompleted(string name)
    {
        _dispatcher.Invoke(() =>
        {
            ReceiveCompleted.Add($"{DateTime.Now:HH:mm:ss} {name}");
            ReceiveProgress = 0;
            ReceiveProgressText = "";
            _ = LoadRecordsAsync();
        });
    }

    private void OnReceiveLogged(string level, string msg)
        => _events.Publish(new LogEvent(level, msg));

    // 说明：接收助手本域与 UI 线程（Dispatcher 回调、MessageBox 确认、Receive* 绑定属性）耦合较深，
    // 且窗口期/退回路径相互交叠，拆分风险高，故本次拆分不涉及此部分，保持原状。

    // ------------------------------------------------------------------ 发送
    // 说明：发送队列/通道解析（ResolveAssistantOnceAsync）/进度回调与 UI 线程及 ObservableCollection
    // 增删严格耦合，拆分风险大，本次拆分不涉及；仅设备域的助手探测缓存（_assistantCache）与发送共用，
    // 已保证两端读写同一字典实例、语义不变。

    private void EnqueueAndStart(IEnumerable<OppSendJob> jobs)
    {
        _queue.Enqueue(jobs);
        // 队列运行中追加的新任务：按最近一次解析的通道立即标注，避免等待期显示 OPP
        if (_queueUseAssistant is { } ua)
        {
            foreach (var j in jobs)
                j.Channel = ua ? TransferConst.ChannelAssistant : TransferConst.ChannelOpp;
        }
        _speedTracker.Reset();
        ProgressText = "";
        TransferProgress = 0;
        var device = SelectedDevice;
        if (device == null) return;
        _queueDevice = device;
        // 经现有 SafeAsync 安全启动（异常经日志上报），等待过程中延续回到 UI 线程
        SafeAsync(() => StartQueueIfNeeded(device));
    }

    private async Task StartQueueIfNeeded(OppDeviceItem device)
    {
        if (_queue.IsRunning) return;
        // 每次队列运行只解析一次通道（确定性），避免逐任务探测/缓存竞态导致同一批文件走不同通道。
        // 不在线程池线程启动：OppSendQueue.StartAsync 会直接增删绑定 DataGrid 的
        // ObservableCollection<OppSendJob>，且其循环体内 await 均未 ConfigureAwait(false)，
        // 因而从 UI 线程启动后，await 的延续会回到 UI 线程，从而保证集合增删发生在 UI 线程，
        // 避免 WPF CollectionView 对跨线程集合修改抛异常。
        try
        {
            var useAssistant = await ResolveAssistantOnceAsync(device);
            _queueUseAssistant = useAssistant;
            foreach (var job in _queue.Jobs.Where(j => j.Status == OppJobStatus.Pending).ToList())
                job.Channel = useAssistant ? TransferConst.ChannelAssistant : TransferConst.ChannelOpp;
            await _queue.StartAsync(device.Addr, (job, ct) => ProcessJobAsync(job, device, useAssistant, ct));
        }
        catch (Exception ex)
        {
            // 队列启动失败：原实现丢弃 Task 导致静默失败用户无感知，现主动经日志上报
            _queueUseAssistant = null;
            PublishLog(TransferConst.LogError, $"启动发送队列失败：{ex.Message}");
        }
    }

    private async Task<bool> ProcessJobAsync(OppSendJob job, OppDeviceItem device, bool useAssistant, CancellationToken ct)
    {
        try
        {
            job.Channel = useAssistant ? TransferConst.ChannelAssistant : TransferConst.ChannelOpp;
            return useAssistant
                ? job.Kind switch
                {
                    TransferConst.TypeFolder => await _assistantPush.SendFolderAsync(device.Addr, job.SourcePath, ct),
                    TransferConst.TypeText => await _assistantPush.SendTextAsync(device.Addr, job.SourcePath, job.DisplayName, ct),
                    _ => await _assistantPush.SendFileAsync(device.Addr, job.SourcePath, zip: false, ct)
                }
                : job.Kind switch
                {
                    TransferConst.TypeFolder => await _push.SendFolderAsync(device.Addr, job.SourcePath, ct),
                    TransferConst.TypeText => await _push.SendTextAsync(device.Addr, job.SourcePath, job.DisplayName, ct),
                    _ => await _push.SendFileAsync(device.Addr, job.SourcePath, zip: false, ct)
                };
        }
        finally
        {
            _ = LoadRecordsAsync();
        }
    }

    /// <summary>
    /// 发送通道的助手探测缓存判等重要逻辑：与设备域共用 _assistantCache（同一字典实例由
    /// DevicePaneController 与这里的 ResolveAssistantOnceAsync 共同读写），"先写者胜"语义保持。
    /// </summary>
    private async Task<bool> ResolveAssistantOnceAsync(OppDeviceItem device)
    {
        switch (_config.TransferMode)
        {
            case TransferConst.ChannelAssistant:
                return true;
            case TransferConst.ChannelOpp:
                return false;
            default:
                lock (_assistantCache)
                {
                    if (_assistantCache.TryGetValue(device.Addr, out var cached))
                        return cached;
                }
                var has = string.IsNullOrEmpty(device.DeviceId)
                    ? false
                    : await Task.Run(() => AssistantDetector.DeviceHasAssistantAsync(device.DeviceId));
                lock (_assistantCache)
                {
                    // 先写者胜：避免扫描异步探测用过期 false 覆盖已确认的 true
                    if (!_assistantCache.ContainsKey(device.Addr))
                        _assistantCache[device.Addr] = has;
                }
                return has;
        }
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
            Kind = TransferConst.TypeFile,
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
                Kind = TransferConst.TypeFolder,
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
                        Kind = TransferConst.TypeText,
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
                        Kind = TransferConst.TypeFile,
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
            _events.Publish(new LogEvent(TransferConst.LogError, $"读取剪贴板失败：{ex.Message}"));
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
                    Kind = TransferConst.TypeFolder,
                    SourcePath = p,
                    DisplayName = System.IO.Path.GetFileName(p.TrimEnd('\\', '/')),
                    Size = 0
                }
                : new OppSendJob
                {
                    Kind = TransferConst.TypeFile,
                    SourcePath = p,
                    DisplayName = System.IO.Path.GetFileName(p),
                    Size = System.IO.File.Exists(p) ? new System.IO.FileInfo(p).Length : 0
                }).ToList();
        EnqueueAndStart(jobs);
        StatusText = $"已加入队列 {jobs.Count} 项：{device.DisplayName}";
        return Task.CompletedTask;
    }

    // ------------------------------------------------------------------ 记录
    // 记录域的纯内部逻辑（加载/导出/清空/统计）已下沉到 RecordsPaneController（含加载并发保护），
    // 以下方法仅作门面委托，保持命令语义与 Records 集合绑定不变。

    private Task LoadRecordsAsync() => _recordsPane.LoadRecordsAsync();

    private Task ExportRecordsAsync(string filter, string ext, Func<List<TransferRecord>, string, string> export)
        => _recordsPane.ExportRecordsAsync(filter, ext, export);

    private Task ClearRecordsAsync() => _recordsPane.ClearRecordsAsync();

    // ------------------------------------------------------------------ 事件
    // 说明：日志/进度事件需经 EventBus 汇集并回写 UI 线程（LogLines/LastError/TransferProgress），
    // 且与 EventBus 订阅生命周期（Dispose 精确退订）强绑定，拆分风险大，本次拆分不涉及。

    private void OnLog(LogEvent e)
    {
        _dispatcher.Invoke(() =>
        {
            if (e.Level == TransferConst.LogError || e.Level == TransferConst.LogWarn)
                _lastError = e.Message;
            var line = $"[{DateTime.Now:HH:mm:ss}] [{e.Level}] {e.Message}";
            LogLines.Add(line);
            if (LogLines.Count > 2000) LogLines.RemoveAt(0);
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
            _events.Publish(new LogEvent(TransferConst.LogError, $"操作失败：{ex.Message}"));
            StatusText = $"错误：{ex.Message}";
        }
    }

    // ------------------------------------------------------------------ 清理

    /// <summary>
    /// 释放路径（应用退出前由 MainWindow.OnTrayExit 调用）：
    /// 1. 退订 EventBus 的日志/进度订阅（避免 handler 连同 this 被长期持有）；
    /// 2. 退订接收服务的三个事件（ProgressChanged/Completed/Logged）；
    /// 3. 若正在监听，停止接收监听（同步等待，异常仅记录、不抛出）。
    /// Dispose 仅做清理，不改变正常运行与退出前的语义。
    /// </summary>
    public void Dispose()
    {
        // 1. EventBus 退订：Unsubscribe 需要与订阅时同一个委托实例，故用保存的 handler 引用。
        _events.Unsubscribe(_logHandler);
        _events.Unsubscribe(_progressHandler);

        // 2. 接收服务事件退订：具名方法生成的委托实例，-= 精确移除捕获 this 的 lambda。
        _receive.ProgressChanged -= OnReceiveProgress;
        _receive.Completed -= OnReceiveCompleted;
        _receive.Logged -= OnReceiveLogged;

        // 3. 停止接收监听（若正在监听）：同步等待结束，异常仅记录、不向调用方抛出。
        if (_receive.IsListening)
        {
            try
            {
                _receive.StopAsync().GetAwaiter().GetResult();
            }
            catch (Exception ex)
            {
                _events.Publish(new LogEvent(TransferConst.LogError, $"退出时停止接收监听失败：{ex.Message}"));
            }
        }
        // 本 ViewModel 无自有 _cts 需要释放（接收服务的 _cts 已由 ReceiveService.StopAsync 内部释放）；
        // 若将来新增后台任务令牌，可在此 _cts?.Dispose() 以尽早释放取消令牌。
    }
}
