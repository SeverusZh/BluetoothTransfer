using System.IO;
using System.Text.Json;

namespace BluetoothTransfer.Models;

public class AppConfig
{
    public string RecvDirectory { get; set; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.MyDocuments),
        "BluetoothTransfer", "recv");
    public string Theme { get; set; } = "light";
    public bool NotificationSound { get; set; } = true;
    public bool AutoConnectLast { get; set; } = true;
    public bool AutoCopyClipboard { get; set; } = true;
    public bool CompressionEnabled { get; set; } = false;
    public bool EncryptionEnabled { get; set; } = true;
    public int AutoCleanDays { get; set; } = 0;
    public int BleMtu { get; set; } = 180;
    public int RfcommChunkSize { get; set; } = 4096;
    public bool AlwaysOnTop { get; set; } = false;
    public string GlobalHotkey { get; set; } = "Ctrl+Shift+B";

    private static readonly string ConfigDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "BluetoothTransfer");
    private static readonly string ConfigPath = Path.Combine(ConfigDir, "config.json");

    public static AppConfig Load()
    {
        try
        {
            if (File.Exists(ConfigPath))
            {
                var json = File.ReadAllText(ConfigPath);
                return JsonSerializer.Deserialize<AppConfig>(json) ?? new AppConfig();
            }
        }
        catch { }
        return new AppConfig();
    }

    public void Save()
    {
        try
        {
            Directory.CreateDirectory(ConfigDir);
            var json = JsonSerializer.Serialize(this, new JsonSerializerOptions { WriteIndented = true });
            File.WriteAllText(ConfigPath, json);
        }
        catch { }
    }
}
