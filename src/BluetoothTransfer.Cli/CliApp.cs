namespace BluetoothTransfer.Cli;

public static class CliApp
{
    public static async Task<int> RunAsync(string[] args)
    {
        var a = Args.Parse(args);
        var command = (a.Get(0) ?? "help").ToLowerInvariant();
        try
        {
            return command switch
            {
                "help" or "-h" or "--help" => Help.Show(),
                "opp-scan" => await Commands.OppScanAsync(a),
                "opp-pair" => await Commands.OppPairAsync(a),
                "opp-send-file" => await Commands.OppSendFileAsync(a),
                "opp-send-files" => await Commands.OppSendFilesAsync(a),
                "opp-send-text" => await Commands.OppSendTextAsync(a),
                "opp-send-folder" => await Commands.OppSendFolderAsync(a),
                "config" => Commands.Config(a),
                "records" => Commands.Records(a),
                "export" => Commands.Export(a),
                "stats" => Commands.Stats(a),
                "devices" => Commands.Devices(a),
                "selftest" => await Commands.SelftestAsync(a),
                "repl" => await Commands.ReplAsync(a),
                _ => Help.Unknown(command)
            };
        }
        catch (Exception ex)
        {
            Console.Error.WriteLine($"[ERROR] {ex.Message}");
            return 1;
        }
    }
}
