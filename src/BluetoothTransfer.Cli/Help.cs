namespace BluetoothTransfer.Cli;

public static class Help
{
    public static int Show()
    {
        Console.WriteLine("""
            btcli —— BluetoothTransfer 命令行工具
            与桌面应用共享同一套核心服务（OBEX OPP/存储/设备管理），
            用于脚本自动化与真机交互测试。

            用法：btcli <命令> [参数]

            通用推送（OPP：仅发送端需要本应用，接收端任意支持"蓝牙文件接收"的设备）
              opp-scan [--seconds 秒] [--paired-only] [--json] [--db 路径]
                                    扫描支持"蓝牙文件接收"（OPP）的经典蓝牙设备
              opp-pair <地址> [--pin 1234] [--db 路径]
                                    发起配对（无 PIN 时走系统确认流程）
              opp-send-file <地址> <文件> [--zip] [--db 路径]
                                    向任意支持 OPP 的设备推送文件（无需对方运行本应用）
                                     --zip 表示先打包为 zip 再推送；
                                     --mode auto|assistant|opp 选择通道（默认 opp）
              opp-send-files <地址> <文件1> [文件2 ...] [--zip] [--db 路径]
                                    批量推送多个文件并汇总成功/失败（同样支持 --mode）
              opp-send-text <地址> <文本> [--name 文件名] [--db 路径]
                                    文本包装为 .txt 后推送（同样支持 --mode）
              opp-send-folder <地址> <文件夹> [--db 路径]
                                    文件夹压缩为 .zip 后推送（同样支持 --mode）
              detect <地址> [--db 路径]
                                    探测对端是否运行接收助手（自定义 UUID）
              package-receiver [--root 仓库根]
                                    一键发布接收助手（自包含 + 框架依赖，可选压缩包）

            记录 / 配置
              records [--direction send|recv] [--type text|file] [--status ok|failed]
                      [--peer 地址] [--search 关键词] [--limit 数量] [--json] [--db 路径]
              records clear [--yes] [--db 路径]
              export <csv|json> <输出文件> [--db 路径]
              stats [--json] [--db 路径]
              devices [--json] [--sort last|name|favorite] [--db 路径]
              devices favorite <地址> [--unset] [--db 路径]
              devices alias <地址> <别名> [--db 路径]
              config                                   查看配置
              config set <key> <value>                 修改配置并保存
                      （OppChunkSize / OppConnectTimeout / OppSendTimeout /
                        PushTextFileName / OppAuthPassword / OppNameUseBom /
                        OppProtectionLevel / OppRetryCount / OppRetryDelaySeconds /
                        TransferMode（auto/assistant/opp））

            测试
              selftest                                 无蓝牙硬件自检：
                        OBEX 编解码与假传输流程、存储与导出
              repl                                     交互式会话（推荐用于模拟真实交互）

            示例：
              btcli opp-scan --seconds 5
              btcli opp-send-file 00:11:22:33:44:55 C:\tmp\photo.jpg
              btcli opp-send-file 00:11:22:33:44:55 C:\tmp\photo.jpg --mode auto
              btcli detect 00:11:22:33:44:55
              btcli opp-send-files 00:11:22:33:44:55 a.pdf b.pdf --zip
              btcli devices alias 00:11:22:33:44:55 我的手机
              btcli records --direction recv --json
              btcli repl
            """);
        return 0;
    }

    public static int Unknown(string command)
    {
        Console.Error.WriteLine($"[ERROR] 未知命令：{command}");
        Show();
        return 2;
    }
}
