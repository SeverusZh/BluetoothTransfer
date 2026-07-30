namespace BluetoothTransfer.Models;

public class TransferRecord
{
    public long Id { get; set; }
    public string CreatedAt { get; set; } = DateTime.Now.ToString("o");
    public string Direction { get; set; } = "send";
    public string Type { get; set; } = "text";
    public string PeerName { get; set; } = "";
    public string PeerAddr { get; set; } = "";
    public string Name { get; set; } = "";
    public long Size { get; set; }
    public string Status { get; set; } = "ok";
    public string Checksum { get; set; } = "";
    public string Channel { get; set; } = "ble";
    public string LocalPath { get; set; } = "";
    public string Note { get; set; } = "";
}
