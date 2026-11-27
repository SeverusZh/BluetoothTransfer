using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Runtime.Versioning;
using Microsoft.Win32;

namespace BluetoothTransfer.Services;

/// <summary>
/// Windows 资源管理器右键菜单 + 「发送到」快捷方式的安装/卸载服务（Phase 1，仅写 HKCU，不申请提权）。
///
/// 安装内容（全部用 <see cref="ShellSendSyntax"/> 生成命令，应用侧解析同一份参数名）：
/// <list type="bullet">
/// <item><c>HKCU\Software\Classes\*\shell\BluetoothTransfer.Send</c>：任意文件，<c>--send "%1"</c></item>
/// <item><c>HKCU\Software\Classes\Directory\shell\BluetoothTransfer.Send</c>：文件夹，<c>--send "%1"</c></item>
/// <item><c>HKCU\Software\Classes\Directory\Background\shell\BluetoothTransfer.Send</c>：文件夹空白处，<c>--send "%V"</c></item>
/// <item><c>HKCU\Software\Classes\Drive\shell\BluetoothTransfer.Send</c>：驱动器，<c>--send "%1"</c></item>
/// <item><c>HKCU\Software\Classes\SystemFileAssociations\.txt\shell\BluetoothTransfer.SendText</c>：.txt 文本内容，<c>--send-text "%1"</c></item>
/// <item><c>%APPDATA%\Microsoft\Windows\SendTo\蓝牙传输.lnk</c>：多选入口（目标=exe，参数=--send）</item>
/// </list>
///
/// 设计约束：
/// 1) 只写 HKCU（HKEY_CURRENT_USER），无需管理员权限，不写 HKLM、不注入资源管理器、不注册 COM 组件；
/// 2) exe 路径取自 <see cref="Environment.ProcessPath"/>（为空时用入口程序集名 + <see cref="AppContext.BaseDirectory"/> 兜底），命令/图标全部加双引号；
/// 3) 「发送到」快捷方式用 WScript.Shell 晚期绑定（不引入新 NuGet，也不要求 COM 引用）；
/// 4) 所有操作返回 (bool 成功, string 中文结果)，失败不静默；安装中途失败会回滚本次已写入的项；
/// 5) 卸载只删本服务创建的 <c>BluetoothTransfer.*</c> 键与同名快捷方式，绝不动其它软件注册项。
///
/// Win11 说明：静态 Verb 不会出现在 Windows 11 新式右键菜单，需要在「显示更多选项」（Shift+F10 / 菜单键）
/// 中查看；「发送到」子菜单在新菜单中仍然可用，多选请走 SendTo（静态 Verb 多选行为跨版本不一致）。
/// </summary>
[SupportedOSPlatform("windows")]
public static class ExplorerIntegrationService
{
    private const string ClassesRootPath = @"Software\Classes";

    /// <summary>文件/文件夹/驱动器用 Verb 键名（统一前缀，卸载按完整键路径精确删除）。</summary>
    private const string SendVerbKeyName = "BluetoothTransfer.Send";

    /// <summary>.txt 文本内容 Verb 键名。</summary>
    private const string SendTextVerbKeyName = "BluetoothTransfer.SendText";

    private const string FileVerbSubKey = @"*\shell\" + SendVerbKeyName;
    private const string DirectoryVerbSubKey = @"Directory\shell\" + SendVerbKeyName;
    private const string BackgroundVerbSubKey = @"Directory\Background\shell\" + SendVerbKeyName;
    private const string DriveVerbSubKey = @"Drive\shell\" + SendVerbKeyName;
    private const string TextVerbSubKey = @"SystemFileAssociations\.txt\shell\" + SendTextVerbKeyName;

    private const string FileMenuText = "用蓝牙传输发送";
    private const string BackgroundMenuText = "用蓝牙传输发送当前文件夹";
    private const string DriveMenuText = "用蓝牙传输发送此驱动器";
    private const string TextMenuText = "用蓝牙发送文本内容";

    private const string NonWindowsMessage = "当前系统不是 Windows，资源管理器右键菜单仅在 Windows 上可用。";

    /// <summary>本服务创建的全部 Verb 子键（卸载/回滚按此清单精确删除）。</summary>
    private static readonly string[] VerbSubKeys =
    {
        FileVerbSubKey,
        DirectoryVerbSubKey,
        BackgroundVerbSubKey,
        DriveVerbSubKey,
        TextVerbSubKey
    };

    /// <summary>
    /// 安装右键菜单与「发送到」快捷方式。
    /// 返回的 <c>Message</c> 为可直接展示的中文结果（成功时含 Win11「显示更多选项」提示）；
    /// 「发送到」快捷方式创建失败但注册表项已写入时仍返回成功，并在结果中说明多选入口不可用。
    /// </summary>
    public static (bool Success, string Message) Install()
    {
        if (!OperatingSystem.IsWindows()) return (false, NonWindowsMessage);

        var exePath = ResolveExecutablePath();
        if (string.IsNullOrWhiteSpace(exePath))
            return (false, "安装失败：无法确定当前程序的可执行文件路径（Environment.ProcessPath 与程序集位置均为空）。");
        if (!exePath.EndsWith(".exe", StringComparison.OrdinalIgnoreCase))
            return (false, $"安装失败：当前程序路径不是 .exe（{exePath}），请在发布版可执行文件中安装。");

        try
        {
            var iconValue = ShellSendSyntax.BuildIconValue(exePath);
            var commands = new (string SubKey, string MenuText, string CommandLine)[]
            {
                (FileVerbSubKey, FileMenuText, ShellSendSyntax.BuildSendCommand(exePath, "%1")),
                (DirectoryVerbSubKey, FileMenuText, ShellSendSyntax.BuildSendCommand(exePath, "%1")),
                (BackgroundVerbSubKey, BackgroundMenuText, ShellSendSyntax.BuildSendCommand(exePath, "%V")),
                (DriveVerbSubKey, DriveMenuText, ShellSendSyntax.BuildSendCommand(exePath, "%1")),
                (TextVerbSubKey, TextMenuText, ShellSendSyntax.BuildSendTextCommand(exePath, "%1"))
            };

            using (var classes = Registry.CurrentUser.CreateSubKey(ClassesRootPath, writable: true)
                   ?? throw new InvalidOperationException(@"无法打开 HKCU\Software\Classes（可能被安全软件限制）"))
            {
                foreach (var (subKey, menuText, commandLine) in commands)
                    WriteVerb(classes, subKey, menuText, iconValue, commandLine);
            }

            var win11Hint = OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000)
                ? "Windows 11 请在右键菜单的「显示更多选项」中查看（Shift+F10 / 菜单键）；"
                : "";

            var (shortcutOk, shortcutError) = CreateSendToShortcut(exePath);
            if (!shortcutOk)
            {
                return (true,
                    $"已安装资源管理器右键菜单（文件/文件夹/背景/驱动器/文本）。{win11Hint}" +
                    $"但「发送到」快捷方式创建失败：{shortcutError}（多选发送暂不可用）。");
            }

            return (true,
                $"已安装资源管理器右键菜单（文件/文件夹/背景/驱动器/文本）与「发送到 → 蓝牙传输」。{win11Hint}" +
                "多选文件请用「发送到」；若程序安装路径变化，重新安装即可更新菜单。");
        }
        catch (Exception ex)
        {
            // 安装失败不能静默：回滚本次已写入的键（尽量）并返回中文原因，由托盘/状态栏提示用户。
            Trace.WriteLine($"[ExplorerIntegration] 安装资源管理器右键菜单失败：{ex}");
            var (removed, shortcutRemoved, rollbackErrors) = RemoveAll(log: null);
            var rollback = removed > 0 || shortcutRemoved ? "本次已写入的部分已回滚。" : "";
            if (rollbackErrors.Count > 0)
                rollback += $"回滚时仍有失败项：{string.Join("；", rollbackErrors)}";
            return (false, $"安装资源管理器右键菜单失败：{ex.Message}。{rollback}");
        }
    }

    /// <summary>
    /// 卸载本服务安装的右键菜单与「发送到」快捷方式（只删 <c>BluetoothTransfer.*</c> 键与同名快捷方式）。
    /// 返回的 <c>Message</c> 为可直接展示的中文结果；部分失败时返回 false 并列出失败项。
    /// </summary>
    public static (bool Success, string Message) Uninstall()
    {
        if (!OperatingSystem.IsWindows()) return (false, NonWindowsMessage);

        var (removed, shortcutRemoved, errors) = RemoveAll(log: null);
        if (errors.Count > 0)
        {
            var partial = removed > 0 || shortcutRemoved
                ? $"已移除 {removed} 个菜单项{(shortcutRemoved ? "与「发送到」快捷方式" : "")}，"
                : "";
            return (false, $"移除资源管理器右键菜单时部分失败：{partial}{string.Join("；", errors)}");
        }

        if (removed == 0 && !shortcutRemoved)
            return (true, "未发现已安装的资源管理器右键菜单，无需移除。");

        return (true,
            $"已移除资源管理器右键菜单（{removed} 个菜单项{(shortcutRemoved ? " + 「发送到」快捷方式" : "")}）。");
    }

    /// <summary>
    /// 解析当前程序的可执行文件路径：优先 <see cref="Environment.ProcessPath"/>（普通构建与单文件/框架依赖发布均指向应用 exe）；
    /// 为空时用「入口程序集名 + <see cref="AppContext.BaseDirectory"/>」推导 exe 作为兜底。
    /// 说明：刻意不使用程序集的位置属性——单文件发布下它恒为空，会触发 IL3000 发布警告；
    /// 推导出的路径必须真实存在才返回，均不可得时返回 null，由调用方给出中文失败提示（绝不写入错误路径）。
    /// </summary>
    public static string? ResolveExecutablePath()
    {
        try
        {
            var processPath = Environment.ProcessPath;
            if (!string.IsNullOrWhiteSpace(processPath)) return Path.GetFullPath(processPath);

            // 仅宿主/极端场景的兜底：单文件发布时 AppContext.BaseDirectory 有效，入口程序集名拼 .exe 即可还原可执行文件。
            var entryName = Assembly.GetEntryAssembly()?.GetName().Name;
            if (!string.IsNullOrWhiteSpace(entryName))
            {
                var candidate = Path.Combine(AppContext.BaseDirectory, entryName + ".exe");
                if (File.Exists(candidate)) return Path.GetFullPath(candidate);
            }

            return null;
        }
        catch (Exception ex)
        {
            // 非法路径等极端情况：返回 null 由调用方提示「无法确定程序路径」，不静默使用错误路径写注册表。
            Trace.WriteLine($"[ExplorerIntegration] 解析程序路径失败：{ex.Message}");
            return null;
        }
    }

    /// <summary>写入单个 Verb（显示文本 + 图标 + 单选模型 + 置顶）及其 command 子键。</summary>
    private static void WriteVerb(
        RegistryKey classes, string subKeyPath, string menuText, string iconValue, string commandLine)
    {
        using var verb = classes.CreateSubKey(subKeyPath, writable: true)
            ?? throw new InvalidOperationException($@"创建注册表键失败：HKCU\Software\Classes\{subKeyPath}");
        // 旧版 Shell 读默认值，新版读 MUIVerb：两者都写，保证 Win10/Win11 旧菜单显示一致。
        verb.SetValue(null, menuText);
        verb.SetValue("MUIVerb", menuText);
        verb.SetValue("Icon", iconValue);
        // 静态 Verb 多选行为跨版本不可靠：显式声明 Single，多选入口统一走「发送到」。
        verb.SetValue("MultiSelectModel", "Single");
        verb.SetValue("Position", "Top");

        using var command = verb.CreateSubKey("command", writable: true)
            ?? throw new InvalidOperationException($@"创建注册表键失败：HKCU\Software\Classes\{subKeyPath}\command");
        command.SetValue(null, commandLine);
    }

    /// <summary>删除全部 Verb 键与「发送到」快捷方式，收集错误（供安装回滚与卸载共用）。</summary>
    private static (int Removed, bool ShortcutRemoved, List<string> Errors) RemoveAll(Action<string>? log)
    {
        var errors = new List<string>();
        var removed = 0;

        foreach (var subKey in VerbSubKeys)
        {
            try
            {
                using var classes = Registry.CurrentUser.OpenSubKey(ClassesRootPath, writable: true);
                if (classes == null)
                {
                    errors.Add(@"无法打开 HKCU\Software\Classes");
                    break;
                }
                // 先确认键存在（打开探测句柄后立即关闭），再删除：避免“删除时仍有子键句柄未释放”。
                bool exists;
                using (var existing = classes.OpenSubKey(subKey))
                    exists = existing != null;
                if (!exists) continue; // 不存在视为已清理，不报错
                classes.DeleteSubKeyTree(subKey, throwOnMissingSubKey: false);
                removed++;
            }
            catch (Exception ex)
            {
                var message = $"{subKey}：{ex.Message}";
                errors.Add(message);
                log?.Invoke(message);
                Trace.WriteLine($"[ExplorerIntegration] 删除注册表键失败：{message}");
            }
        }

        var shortcutRemoved = false;
        try
        {
            shortcutRemoved = DeleteSendToShortcut();
        }
        catch (Exception ex)
        {
            var message = $"「发送到」快捷方式：{ex.Message}";
            errors.Add(message);
            log?.Invoke(message);
            Trace.WriteLine($"[ExplorerIntegration] 删除「发送到」快捷方式失败：{ex}");
        }

        return (removed, shortcutRemoved, errors);
    }

    /// <summary>
    /// 创建 <c>%APPDATA%\Microsoft\Windows\SendTo\蓝牙传输.lnk</c>：
    /// Target=exe、Arguments=--send（选中项由资源管理器追加）、图标=exe,0。
    /// 使用 WScript.Shell 晚期绑定，避免引入新 NuGet / COM 互操作引用。
    /// </summary>
    private static (bool Success, string Error) CreateSendToShortcut(string exePath)
    {
        var sendToDirectory = ResolveSendToDirectory();
        if (string.IsNullOrWhiteSpace(sendToDirectory))
            return (false, "无法定位当前用户的「发送到」目录（%APPDATA%\\Microsoft\\Windows\\SendTo）");

        object? shell = null;
        object? shortcut = null;
        try
        {
            Directory.CreateDirectory(sendToDirectory);
            var shortcutPath = Path.Combine(sendToDirectory, ShellSendSyntax.SendToShortcutFileName);
            var shellType = Type.GetTypeFromProgID("WScript.Shell")
                ?? throw new InvalidOperationException("系统中未注册 WScript.Shell COM 组件，无法创建快捷方式");
            shell = Activator.CreateInstance(shellType)
                ?? throw new InvalidOperationException("创建 WScript.Shell 实例失败");

            var createShortcut = shellType.InvokeMember(
                "CreateShortcut", BindingFlags.InvokeMethod, null, shell, new object[] { shortcutPath });
            shortcut = createShortcut
                ?? throw new InvalidOperationException("创建快捷方式对象失败");
            var shortcutType = shortcut.GetType();
            SetComProperty(shortcutType, shortcut, "TargetPath", exePath);
            SetComProperty(shortcutType, shortcut, "Arguments", ShellSendSyntax.SendToArguments);
            SetComProperty(shortcutType, shortcut, "IconLocation", exePath + ",0");
            SetComProperty(shortcutType, shortcut, "WorkingDirectory", Path.GetDirectoryName(exePath) ?? "");
            SetComProperty(shortcutType, shortcut, "Description", "用蓝牙传输发送");
            shortcutType.InvokeMember("Save", BindingFlags.InvokeMethod, null, shortcut, null);
            return (true, "");
        }
        catch (Exception ex)
        {
            // COM 晚期绑定抛出的真实原因在 TargetInvocationException.InnerException 里，优先展示内层消息。
            var message = ex is TargetInvocationException { InnerException: { } inner } ? inner.Message : ex.Message;
            Trace.WriteLine($"[ExplorerIntegration] 创建「发送到」快捷方式失败：{ex}");
            return (false, message);
        }
        finally
        {
            ReleaseComObject(shortcut);
            ReleaseComObject(shell);
        }
    }

    /// <summary>删除「发送到」快捷方式；不存在返回 false（不视为错误）。</summary>
    private static bool DeleteSendToShortcut()
    {
        var sendToDirectory = ResolveSendToDirectory();
        if (string.IsNullOrWhiteSpace(sendToDirectory)) return false;
        var shortcutPath = Path.Combine(sendToDirectory, ShellSendSyntax.SendToShortcutFileName);
        if (!File.Exists(shortcutPath)) return false;
        File.Delete(shortcutPath);
        return true;
    }

    /// <summary>
    /// 「发送到」目录：优先 <see cref="Environment.SpecialFolder.SendTo"/>，
    /// 取不到时按文档路径 <c>%APPDATA%\Microsoft\Windows\SendTo</c> 兜底。
    /// </summary>
    private static string ResolveSendToDirectory()
    {
        var sendTo = Environment.GetFolderPath(Environment.SpecialFolder.SendTo);
        if (!string.IsNullOrWhiteSpace(sendTo)) return sendTo;
        var appData = Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData);
        return string.IsNullOrWhiteSpace(appData)
            ? ""
            : Path.Combine(appData, "Microsoft", "Windows", "SendTo");
    }

    /// <summary>设置 COM 晚期绑定对象的一个字符串属性（失败时抛 TargetInvocationException，由调用方转中文提示）。</summary>
    private static void SetComProperty(Type comType, object target, string propertyName, string value)
        => comType.InvokeMember(propertyName, BindingFlags.SetProperty, null, target, new object[] { value });

    /// <summary>释放 COM 对象；释放失败只影响进程退出时的资源回收，记录诊断输出，不影响安装结果。</summary>
    private static void ReleaseComObject(object? comObject)
    {
        if (comObject == null) return;
        try
        {
            if (Marshal.IsComObject(comObject))
                Marshal.FinalReleaseComObject(comObject);
        }
        catch (Exception ex)
        {
            Trace.WriteLine($"[ExplorerIntegration] 释放 COM 对象失败：{ex.Message}");
        }
    }
}
