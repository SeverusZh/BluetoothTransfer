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
    private readonly ThemeService _themeService;

    private string _statusText = "Ready";
    private bool _isScanning;
    private bool _isConnected;
    private bool _isAdvertising;
    private string _sendText = "";
    private string _receivedText = "";
    private string _logText = "";
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
    public string LogText { get => _logText; set => SetProperty(ref _logText, value); }
    public string DeviceName { get => _deviceName; set => SetProperty(ref _deviceName, value); }
    public DeviceInfo? SelectedDevice { get => _selectedDevice; set { SetProperty(ref _selectedDevice, value); OnPropertyChanged(nameof(CanConnect)); } }
    public string ScanButtonText => IsScanning ? "Stop Scan" : "Scan";
    public string AdvertiseButtonText => IsAdvertising ? "Stop Advertise" : "Advertise";
    public string ThemeButtonText => _themeService.CurrentTheme == "dark" ? "Light Theme" : "Dark Theme";
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
    public RelayCommand ToggleThemeCommand { get; }

    public MainViewModel()
    {
        _dispatcher = Dispatcher.CurrentDispatcher;
        _eventBus = new EventBus();
        _storage = new StorageService();
        _ble = new BleService(_eventBus);
        _gattServer = new BleGattServer(_eventBus);
        _rfcomm = new RfcommChannel(_eventBus);
        _config = AppConfig.Load();
        _fileTransfer = new FileTransferService(_eventBus, _storage, _config, _rfcomm, _ble);
        _themeService = new ThemeService();

        ScanCommand = new RelayCommand(ToggleScan);
        AdvertiseCommand = new RelayCommand(() => SafeAsync(ToggleAdvertiseAsync));
        ConnectCommand = new RelayCommand(() => SafeAsync(ConnectAsync), () => CanConnect);
        DisconnectCommand = new RelayCommand(() => Disconnect(), () => IsConnected);
        SendTextCommand = new RelayCommand(() => SafeAsync(SendTextAsync), () => CanSend);
        SendFileCommand = new RelayCommand(() => SafeAsync(SendFileAsync));
        CopyReceivedCommand = new RelayCommand(() =>
        {
            if (!string.IsNullOrEmpty(ReceivedText))
                Clipboard.SetText(ReceivedText);
        });
        RefreshRecordsCommand = new RelayCommand(LoadRecords);
        ExportCsvCommand = new RelayCommand(ExportCsv);
        ExportJsonCommand = new RelayCommand(ExportJson);
        ToggleThemeCommand = new RelayCommand(() =>
        {
            _themeService.ToggleTheme();
            _config.Theme = _themeService.CurrentTheme;
            _config.Save();
            OnPropertyChanged(nameof(ThemeButtonText));
        });

        _themeService.ApplyTheme(_config.Theme);
        SubscribeEvents();
        LoadRecords();
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
            StatusText = $"Connected: {e.Name}";
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
            StatusText = "Disconnected";
            var dev = Devices.FirstOrDefault(d => d.Addr == e.Addr);
            if (dev != null) dev.IsConnected = false;
        }));

        _eventBus.Subscribe<TextReceivedEvent>(e => _dispatcher.Invoke(() =>
        {
            ReceivedText = e.Text;
            if (_config.AutoCopyClipboard)
                Clipboard.SetText(e.Text);
            _storage.AddRecord(new TransferRecord
            {
                Direction = "recv",
                Type = "text",
                PeerName = e.PeerName,
                PeerAddr = e.Addr,
                Name = e.Text.Length > 50 ? e.Text[..50] + "..." : e.Text,
                Size = System.Text.Encoding.UTF8.GetByteCount(e.Text),
                Status = "ok",
                Channel = "ble"
            });
            LoadRecords();
        }));

        _eventBus.Subscribe<LogEvent>(e => _dispatcher.Invoke(() =>
        {
            var line = $"[{DateTime.Now:HH:mm:ss}] [{e.Level}] {e.Message}";
            LogLines.Add(line);
            if (LogLines.Count > 500) LogLines.RemoveAt(0);
        }));
    }

    private void ToggleScan()
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

    private async Task ConnectAsync()
    {
        if (SelectedDevice == null) return;
        StatusText = $"Connecting to {SelectedDevice.Name}...";
        var ok = await _ble.ConnectAsync(SelectedDevice.Addr);
        if (!ok) StatusText = "Connection failed";
    }

    private void Disconnect()
    {
        _ble.Disconnect();
    }

    private async Task ToggleAdvertiseAsync()
    {
        if (IsAdvertising)
        {
            _gattServer.Stop();
            IsAdvertising = false;
            StatusText = "Advertising stopped";
        }
        else
        {
            var ok = await _gattServer.StartAsync(DeviceName);
            IsAdvertising = ok;
            StatusText = ok ? $"Advertising as \"{DeviceName}\"" : "Failed to start advertising";
        }
    }

    private async Task SendTextAsync()
    {
        if (string.IsNullOrWhiteSpace(SendText)) return;
        var text = SendText;
        var ok = await _ble.SendTextAsync(text);
        if (ok)
        {
            _storage.AddRecord(new TransferRecord
            {
                Direction = "send",
                Type = "text",
                PeerName = _ble.ConnectedName ?? "",
                PeerAddr = _ble.ConnectedAddr ?? "",
                Name = text.Length > 50 ? text[..50] + "..." : text,
                Size = System.Text.Encoding.UTF8.GetByteCount(text),
                Status = "ok",
                Channel = "ble"
            });
            SendText = "";
            StatusText = "Text sent";
            LoadRecords();
        }
        else
        {
            StatusText = "Send failed";
        }
    }

    private async Task SendFileAsync()
    {
        var dialog = new Microsoft.Win32.OpenFileDialog
        {
            Multiselect = true,
            Title = "Select files to send"
        };
        if (dialog.ShowDialog() != true) return;

        StatusText = $"Sending {dialog.FileNames.Length} file(s)...";
        foreach (var file in dialog.FileNames)
        {
            await _fileTransfer.SendFileAsync(file);
        }
        StatusText = "File(s) sent";
        LoadRecords();
    }

    public async Task SendSingleFileAsync(string filePath)
    {
        StatusText = $"Sending {System.IO.Path.GetFileName(filePath)}...";
        await _fileTransfer.SendFileAsync(filePath);
        StatusText = "File sent";
        LoadRecords();
    }

    public async Task SendFolderAsync(string folderPath)
    {
        StatusText = $"Sending folder...";
        await _fileTransfer.SendFolderAsync(folderPath);
        StatusText = "Folder sent";
        LoadRecords();
    }

    private void LoadRecords()
    {
        var records = _storage.GetRecords(limit: 100);
        Records.Clear();
        foreach (var r in records) Records.Add(r);
    }

    private void ExportCsv()
    {
        var dialog = new Microsoft.Win32.SaveFileDialog
        {
            Filter = "CSV files|*.csv",
            FileName = $"transfer_records_{DateTime.Now:yyyyMMdd_HHmmss}.csv"
        };
        if (dialog.ShowDialog() != true) return;
        var records = _storage.GetRecords(limit: 10000);
        ExportService.ExportCsv(records, dialog.FileName);
        StatusText = $"Exported {records.Count} records to CSV";
    }

    private void ExportJson()
    {
        var dialog = new Microsoft.Win32.SaveFileDialog
        {
            Filter = "JSON files|*.json",
            FileName = $"transfer_records_{DateTime.Now:yyyyMMdd_HHmmss}.json"
        };
        if (dialog.ShowDialog() != true) return;
        var records = _storage.GetRecords(limit: 10000);
        ExportService.ExportJson(records, dialog.FileName);
        StatusText = $"Exported {records.Count} records to JSON";
    }

    private async void SafeAsync(Func<Task> action)
    {
        try
        {
            await action();
        }
        catch (Exception ex)
        {
            _eventBus.Publish(new LogEvent("ERROR", $"Command failed: {ex.Message}"));
            StatusText = $"Error: {ex.Message}";
        }
    }
}
