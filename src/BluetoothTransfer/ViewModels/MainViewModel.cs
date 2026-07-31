using System.Collections.ObjectModel;
using System.Windows;
using System.Windows.Threading;
using BluetoothTransfer.Models;
using BluetoothTransfer.Services;

namespace BluetoothTransfer.ViewModels;

public class MainViewModel : ViewModelBase
{
    private readonly BleService _ble;
    private readonly BleGattServer _gattServer;
    private readonly RfcommChannel _rfcomm;
    private readonly FileTransferService _fileTransfer;
    private readonly StorageService _storage;
    private readonly EventBus _eventBus;
    private readonly AppConfig _config;
    private readonly Dispatcher _dispatcher;
    private readonly CryptoService _crypto;
    private readonly FrameReassembler _reassembler;

    private string _statusText = "就绪";
    private bool _isScanning;
    private bool _isConnected;
    private bool _isAdvertising;
    private string _sendText = "";
    private string _receivedText = "";
    private DeviceInfo? _selectedDevice;
    private string _deviceName = Environment.MachineName;
    private double _transferProgress;

    public ObservableCollection<DeviceInfo> Devices { get; } = new();
    public ObservableCollection<TransferRecord> Records { get; } = new();
    public ObservableCollection<string> LogLines { get; } = new();

    public string StatusText { get => _statusText; set => SetProperty(ref _statusText, value); }
    public bool IsScanning { get => _isScanning; set { SetProperty(ref _isScanning, value); OnPropertyChanged(nameof(ScanButtonText)); } }
    public bool IsConnected { get => _isConnected; set { SetProperty(ref _isConnected, value); OnPropertyChanged(nameof(CanSend)); } }
    public bool IsAdvertising { get => _isAdvertising; set { SetProperty(ref _isAdvertising, value); OnPropertyChanged(nameof(AdvertiseButtonText)); } }
    public string SendText { get => _sendText; set => SetProperty(ref _sendText, value); }
    public string ReceivedText { get => _receivedText; set => SetProperty(ref _receivedText, value); }
    public string DeviceName { get => _deviceName; set => SetProperty(ref _deviceName, value); }
    public DeviceInfo? SelectedDevice { get => _selectedDevice; set { SetProperty(ref _selectedDevice, value); OnPropertyChanged(nameof(CanConnect)); } }
    public string ScanButtonText => IsScanning ? "停止扫描" : "扫描";
    public string AdvertiseButtonText => IsAdvertising ? "停止广播" : "广播";
    public bool CanSend => IsConnected && !string.IsNullOrWhiteSpace(SendText);
    public bool CanConnect => SelectedDevice != null && !IsConnected;
    public double TransferProgress { get => _transferProgress; set => SetProperty(ref _transferProgress, value); }

    public RelayCommand ScanCommand { get; }
    public RelayCommand AdvertiseCommand { get; }
    public RelayCommand ConnectCommand { get; }
    public RelayCommand DisconnectCommand { get; }
    public RelayCommand SendTextCommand { get; }
    public RelayCommand SendFileCommand { get; }
    public RelayCommand CopyReceivedCommand { get; }
    public RelayCommand RefreshRecordsCommand { get; }
    public RelayCommand ExportCsvCommand { get; }
    public RelayCommand ExportJsonCommand { get; }

    public MainViewModel()
    {
        _dispatcher = Dispatcher.CurrentDispatcher;
        _eventBus = new EventBus();
        _storage = new StorageService();
        _config = AppConfig.Load();
        _crypto = new CryptoService();
        _reassembler = new FrameReassembler(_eventBus, _crypto);
        _ble = new BleService(_eventBus, _crypto, _reassembler);
        _gattServer = new BleGattServer(_eventBus, _crypto, _reassembler);
        _rfcomm = new RfcommChannel(_eventBus, _storage, _crypto, _config);
        _fileTransfer = new FileTransferService(_eventBus, _storage, _config, _rfcomm, _ble, _crypto);

        ScanCommand = new RelayCommand(ToggleScan);
        AdvertiseCommand = new RelayCommand(() => SafeAsync(ToggleAdvertiseAsync));
        ConnectCommand = new RelayCommand(() => SafeAsync(ConnectAsync), () => CanConnect);
        DisconnectCommand = new RelayCommand(() => Disconnect(), () => IsConnected);
        SendTextCommand = new RelayCommand(() => SafeAsync(SendTextAsync), () => CanSend);
        SendFileCommand = new RelayCommand(() => SafeAsync(SendFileAsync));
        CopyReceivedCommand = new RelayCommand(() =>
        {
            try
            {
                if (!string.IsNullOrEmpty(ReceivedText))
                    Clipboard.SetText(ReceivedText);
            }
            catch (Exception ex)
            {
                _eventBus.Publish(new LogEvent("ERROR", $"复制到剪贴板失败：{ex.Message}"));
            }
        });
        RefreshRecordsCommand = new RelayCommand(() => SafeAsync(LoadRecordsAsync));
        ExportCsvCommand = new RelayCommand(ExportCsv);
        ExportJsonCommand = new RelayCommand(ExportJson);

        SubscribeEvents();
        _ = LoadRecordsAsync();
    }

    private void SubscribeEvents()
    {
        _eventBus.Subscribe<DeviceDiscoveredEvent>(e => _dispatcher.Invoke(() =>
        {
            var existing = Devices.FirstOrDefault(d => d.Addr == e.Addr);
            if (existing != null)
            {
                existing.Rssi = e.Rssi;
                existing.LastSeen = DateTime.Now.ToString("o");
            }
            else
            {
                Devices.Add(new DeviceInfo { Addr = e.Addr, Name = e.Name, Rssi = e.Rssi, LastSeen = DateTime.Now.ToString("o") });
            }
        }));

        _eventBus.Subscribe<DeviceConnectedEvent>(e => _dispatcher.Invoke(() =>
        {
            IsConnected = true;
            StatusText = $"已连接：{e.Name}";
            var dev = Devices.FirstOrDefault(d => d.Addr == e.Addr);
            if (dev != null) dev.IsConnected = true;
            _storage.UpsertDevice(new DeviceInfo
            {
                Addr = e.Addr,
                Name = e.Name,
                LastSeen = DateTime.Now.ToString("o"),
                LastConnected = DateTime.Now.ToString("o")
            });
        }));

        _eventBus.Subscribe<DeviceDisconnectedEvent>(e => _dispatcher.Invoke(() =>
        {
            IsConnected = false;
            StatusText = "已断开连接";
            var dev = Devices.FirstOrDefault(d => d.Addr == e.Addr);
            if (dev != null) dev.IsConnected = false;
        }));

        _eventBus.Subscribe<TextReceivedEvent>(e => _dispatcher.Invoke(() =>
        {
            ReceivedText = e.Text;
            if (_config.AutoCopyClipboard)
                Clipboard.SetText(e.Text);
            _storage.AddRecord(MakeTextRecord(TransferConst.DirRecv, e.PeerName, e.Addr, e.Text));
            _ = LoadRecordsAsync();
        }));

        _eventBus.Subscribe<LogEvent>(e => _dispatcher.Invoke(() =>
        {
            var line = $"[{DateTime.Now:HH:mm:ss}] [{e.Level}] {e.Message}";
            LogLines.Add(line);
            if (LogLines.Count > 500) LogLines.RemoveAt(0);
        }));

        _eventBus.Subscribe<FileReceivedEvent>(e => _dispatcher.Invoke(() =>
        {
            StatusText = $"已接收文件：{e.FileName}";
            _ = LoadRecordsAsync();
        }));

        _eventBus.Subscribe<TransferProgressEvent>(e => _dispatcher.Invoke(() =>
        {
            TransferProgress = e.TotalBytes > 0 ? (double)e.BytesSent / e.TotalBytes * 100.0 : 0;
        }));
    }

    private void ToggleScan()
    {
        try
        {
            if (IsScanning)
            {
                _ble.StopScan();
                IsScanning = false;
            }
            else
            {
                Devices.Clear();
                _ble.StartScan();
                IsScanning = true;
            }
        }
        catch (Exception ex)
        {
            IsScanning = false;
            _eventBus.Publish(new LogEvent("ERROR", $"扫描失败：{ex.Message}"));
            StatusText = $"错误：{ex.Message}";
        }
    }

    private async Task ConnectAsync()
    {
        if (SelectedDevice == null) return;
        StatusText = $"正在连接到 {SelectedDevice.Name}...";
        var ok = await _ble.ConnectAsync(SelectedDevice.Addr);
        if (!ok)
        {
            StatusText = "连接失败";
            return;
        }

        var rfcommOk = await _rfcomm.ConnectToServerAsync(SelectedDevice.Name);
        if (!rfcommOk)
        {
            StatusText = "BLE 已连接，但 RFCOMM 通道连接失败（大文件传输不可用）";
            _eventBus.Publish(new LogEvent("WARN", "RFCOMM 通道连接失败，大于 10KB 的文件将无法发送"));
        }
    }

    private void Disconnect()
    {
        _rfcomm.Close();
        _ble.Disconnect();
    }

    private async Task ToggleAdvertiseAsync()
    {
        if (IsAdvertising)
        {
            _gattServer.Stop();
            _rfcomm.Close();
            IsAdvertising = false;
            StatusText = "已停止广播";
        }
        else
        {
            var gattOk = await _gattServer.StartAsync(DeviceName);
            var ok = gattOk && await _rfcomm.StartServerAsync();
            if (!ok && gattOk)
            {
                // GATT 已启动但 RFCOMM 失败：回滚已创建的服务，避免半启动状态。
                _gattServer.Stop();
                _eventBus.Publish(new LogEvent("ERROR", "RFCOMM 服务端启动失败"));
            }
            else if (!ok)
            {
                _eventBus.Publish(new LogEvent("ERROR", "GATT 服务端启动失败"));
            }
            IsAdvertising = ok;
            StatusText = ok
                ? _gattServer.IsNameAdvertised
                    ? $"正在广播：\"{DeviceName}\""
                    : $"正在广播：\"{DeviceName}\"（本机名称广播不可用，可按地址连接）"
                : "启动广播失败";
        }
    }

    private async Task SendTextAsync()
    {
        if (string.IsNullOrWhiteSpace(SendText)) return;
        var text = SendText;
        var ok = await _ble.SendTextAsync(text);
        if (ok)
        {
            _storage.AddRecord(MakeTextRecord(TransferConst.DirSend, _ble.ConnectedName ?? "", _ble.ConnectedAddr ?? "", text));
            SendText = "";
            StatusText = "文本已发送";
            await LoadRecordsAsync();
        }
        else
        {
            StatusText = "发送失败";
        }
    }

    private async Task SendFileAsync()
    {
        var dialog = new Microsoft.Win32.OpenFileDialog
        {
            Multiselect = true,
            Title = "选择要发送的文件"
        };
        if (dialog.ShowDialog() != true) return;

        StatusText = $"正在发送 {dialog.FileNames.Length} 个文件...";
        var okCount = 0;
        foreach (var file in dialog.FileNames)
        {
            if (await _fileTransfer.SendFileAsync(file))
                okCount++;
        }
        StatusText = okCount == dialog.FileNames.Length
            ? $"已发送 {okCount} 个文件"
            : $"发送完成：成功 {okCount}/{dialog.FileNames.Length}";
        await LoadRecordsAsync();
    }

    public async Task SendSingleFileAsync(string filePath)
    {
        StatusText = $"正在发送 {System.IO.Path.GetFileName(filePath)}...";
        var ok = await _fileTransfer.SendFileAsync(filePath);
        StatusText = ok ? "文件已发送" : "文件发送失败";
        await LoadRecordsAsync();
    }

    public async Task SendFolderAsync(string folderPath)
    {
        StatusText = "正在发送文件夹...";
        var ok = await _fileTransfer.SendFolderAsync(folderPath);
        StatusText = ok ? "文件夹已发送" : "文件夹发送失败";
        await LoadRecordsAsync();
    }

    private async Task LoadRecordsAsync()
    {
        var records = await Task.Run(() => _storage.GetRecords(limit: 100));
        _dispatcher.Invoke(() =>
        {
            Records.Clear();
            foreach (var r in records) Records.Add(r);
        });
    }

    private void ExportCsv() => ExportRecords("CSV 文件|*.csv", "csv", ExportService.ExportCsv);

    private void ExportJson() => ExportRecords("JSON 文件|*.json", "json", ExportService.ExportJson);

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
            // 同步命令无 SafeAsync 兜底，异常须就地捕获，否则会逃逸到 WPF 导致应用崩溃。
            _eventBus.Publish(new LogEvent("ERROR", $"导出失败：{ex.Message}"));
            StatusText = $"导出失败：{ex.Message}";
        }
    }

    private static TransferRecord MakeTextRecord(string direction, string peerName, string peerAddr, string text)
    {
        return new TransferRecord
        {
            Direction = direction,
            Type = TransferConst.TypeText,
            PeerName = peerName,
            PeerAddr = peerAddr,
            Name = text.Length > 50 ? text[..50] + "..." : text,
            Size = System.Text.Encoding.UTF8.GetByteCount(text),
            Status = TransferConst.StatusOk,
            Channel = TransferConst.ChannelBle
        };
    }

    private async void SafeAsync(Func<Task> action)
    {
        try
        {
            await action();
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"命令执行失败：{ex.Message}"));
            StatusText = $"错误：{ex.Message}";
        }
    }
}
