using System.ComponentModel;
using System.Runtime.CompilerServices;

namespace BluetoothTransfer.Models;

public class DeviceInfo : INotifyPropertyChanged
{
    private string _addr = "";
    private string _name = "";
    private string _alias = "";
    private bool _favorite;
    private string _lastSeen = "";
    private string _lastConnected = "";
    private int _rssi;
    private bool _isConnected;

    public event PropertyChangedEventHandler? PropertyChanged;

    private void SetProperty<T>(ref T field, T value, [CallerMemberName] string? name = null)
    {
        if (EqualityComparer<T>.Default.Equals(field, value)) return;
        field = value;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
    }

    public string Addr { get => _addr; set => SetProperty(ref _addr, value); }
    public string Name { get => _name; set => SetProperty(ref _name, value); }
    public string Alias { get => _alias; set => SetProperty(ref _alias, value); }
    public bool Favorite { get => _favorite; set => SetProperty(ref _favorite, value); }
    public string LastSeen { get => _lastSeen; set => SetProperty(ref _lastSeen, value); }
    public string LastConnected { get => _lastConnected; set => SetProperty(ref _lastConnected, value); }
    public int Rssi { get => _rssi; set => SetProperty(ref _rssi, value); }
    public bool IsConnected { get => _isConnected; set => SetProperty(ref _isConnected, value); }
}
