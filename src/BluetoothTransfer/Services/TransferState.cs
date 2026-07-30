using System.IO;
using System.Text.Json;

namespace BluetoothTransfer.Services;

public class TransferState
{
    public uint TaskId { get; set; }
    public string FileName { get; set; } = "";
    public string PartialPath { get; set; } = "";
    public long TotalSize { get; set; }
    public long ReceivedBytes { get; set; }
    public string Checksum { get; set; } = "";
    public bool Compressed { get; set; }
    public string CreatedAt { get; set; } = DateTime.Now.ToString("o");

    private static readonly string StateDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "BluetoothTransfer", "partial");

    public static string GetPartialDir()
    {
        Directory.CreateDirectory(StateDir);
        return StateDir;
    }

    public void Save()
    {
        Directory.CreateDirectory(StateDir);
        var path = Path.Combine(StateDir, $"{TaskId}.json");
        File.WriteAllText(path, JsonSerializer.Serialize(this));
    }

    public static TransferState? Load(uint taskId)
    {
        var path = Path.Combine(StateDir, $"{taskId}.json");
        if (!File.Exists(path)) return null;
        try
        {
            return JsonSerializer.Deserialize<TransferState>(File.ReadAllText(path));
        }
        catch { return null; }
    }

    public static List<TransferState> LoadAll()
    {
        Directory.CreateDirectory(StateDir);
        var states = new List<TransferState>();
        foreach (var file in Directory.GetFiles(StateDir, "*.json"))
        {
            try
            {
                var state = JsonSerializer.Deserialize<TransferState>(File.ReadAllText(file));
                if (state != null) states.Add(state);
            }
            catch { }
        }
        return states;
    }

    public void Delete()
    {
        var path = Path.Combine(StateDir, $"{TaskId}.json");
        if (File.Exists(path)) File.Delete(path);
        if (File.Exists(PartialPath)) File.Delete(PartialPath);
    }
}
