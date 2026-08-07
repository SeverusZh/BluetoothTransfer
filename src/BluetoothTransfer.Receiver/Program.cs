using System.Runtime.InteropServices;
using System.IO;
using BluetoothTransfer.Receiver;

var command = args.Length == 0 ? "gui" : args[0].ToLowerInvariant();
#if CLI_ONLY
// 纯 CLI 形态没有 GUI 壳：无参数/双击直接进入 run（接收监听）
if (command is "gui" or "--gui") command = "run";
#endif
#if !CLI_ONLY
if (command is "gui" or "--gui")
{
    HideConsoleWindow();
    var app = new System.Windows.Application();
    var saveDir = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        "Downloads", "BluetoothReceive");
    app.Run(new ReceiverWindow(saveDir, ask: false));
    return 0;
}
#endif
return await ReceiverCli.RunAsync(args);

#if !CLI_ONLY
[DllImport("user32.dll")]
static extern IntPtr GetConsoleWindow();

[DllImport("user32.dll")]
static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

static void HideConsoleWindow()
{
    try { ShowWindow(GetConsoleWindow(), 0); } catch { }
}
#endif
