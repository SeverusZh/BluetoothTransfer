namespace BluetoothTransfer.Models;

public class DeviceInfo
{
    public string Addr { get; set; } = "";
    public string Name { get; set; } = "";
    public string Alias { get; set; } = "";
    public bool Favorite { get; set; }
    public string LastSeen { get; set; } = "";
    public string LastConnected { get; set; } = "";
    public int Rssi { get; set; }
    public bool IsConnected { get; set; }
}
