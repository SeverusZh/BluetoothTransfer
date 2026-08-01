namespace BluetoothTransfer.Cli;

public static class Help
{
    public static int Show()
    {
        Console.WriteLine("""
            btcli —— BluetoothTransfer 命令行工具
            与桌面应用共享同一套核心服务（BLE/RFCOMM/协议/加密/存储），
            用于脚本自动化与双机真实交互测试。

            用法：btcli <命令> [参数]

            扫描 / 服务端
              scan [--seconds 秒] [--json] [--db 路径]
                                    扫描周边 BLE 设备
              serve [--name 名称] [--ble-only] [--timeout 秒] [--json] [--db 路径]
                                    启动接收端：BLE 广播 + RFCOMM 服务端，
                                    实时显示收到的文本/文件（Ctrl+C 退出）
              connect <地址> [--peer 名称] [--no-rfcomm] [--timeout 秒] [--json] [--db 路径]
                                    连接对端并保持会话，实时显示对端发来的数据

            发送
              send-text <地址> <文本> [--peer 名称] [--db 路径]
                                    发送文本（走 BLE 通道）
              send-file <地址> <文件> [--peer 名称]
                        [--compress|--no-compress] [--encrypt|--no-encrypt]
                        [--chunk 字节] [--db 路径]
                                    发送文件（>10KB 自动走 RFCOMM，小文件走 BLE）
              send-folder <地址> <文件夹> [同上选项]
                                    递归发送文件夹内全部文件

            通用推送（OPP，1.1 新增：仅发送端需要本应用）
              opp-scan [--seconds 秒] [--paired-only] [--json] [--db 路径]
                                    扫描支持"蓝牙文件接收"（OPP）的经典蓝牙设备
              opp-pair <地址> [--pin 1234] [--db 路径]
                                    发起配对（无 PIN 时走系统确认流程）
              opp-send-file <地址> <文件> [--db 路径]
                                    向任意支持 OPP 的设备推送文件（无需对方运行本应用）
              opp-send-text <地址> <文本> [--name 文件名] [--db 路径]
                                    文本包装为 .txt 后推送
              opp-send-folder <地址> <文件夹> [--db 路径]
                                    文件夹压缩为 .zip 后推送

            记录 / 配置
             records [--direction send|recv] [--type text|file] [--status ok|failed]
                     [--peer 地址] [--search 关键词] [--limit 数量] [--json] [--db 路径]
             records clear [--yes] [--db 路径]
             export <csv|json> <输出文件> [--db 路径]
              stats [--json] [--db 路径]
              devices [--json] [--db 路径]
              config                                   查看配置
              config set <key> <value>                 修改配置并保存
                      （RecvDirectory / AutoCopyClipboard / CompressionEnabled /
                        EncryptionEnabled / RfcommChunkSize / OppChunkSize /
                        OppConnectTimeout / OppSendTimeout / PushTextFileName /
                        OppAuthPassword）

            测试
              selftest                                 无蓝牙硬件自检：
                        分帧/CRC、ECDH+AES-GCM 加密、Deflate 压缩、存储与导出、
                        OBEX 编解码与假传输流程
              repl                                     交互式会话（推荐用于模拟真实交互）

            示例：
              btcli serve --name TestPC
              btcli scan --seconds 5
              btcli connect A0E9F1D27B3C --peer TestPC
              btcli send-file A0E9F1D27B3C C:\tmp\doc.pdf --compress --encrypt
              btcli opp-scan --seconds 5
              btcli opp-send-file 00:11:22:33:44:55 C:\tmp\photo.jpg
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
