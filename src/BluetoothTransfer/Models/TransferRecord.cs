namespace BluetoothTransfer.Models;

public class TransferRecord
{
    public long Id { get; set; }
    /// <summary>留空时由 StorageService.AddRecord 在入库时取当前时间，避免对象构造到入库间耗时导致时间戳偏早。</summary>
    public string CreatedAt { get; set; } = "";
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
