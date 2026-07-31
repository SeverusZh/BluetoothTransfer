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

            记录 / 配置
              records [--direction send|recv] [--type text|file] [--status ok|failed]
                      [--peer 地址] [--search 关键词] [--limit 数量] [--json] [--db 路径]
              export <csv|json> <输出文件> [--db 路径]
              stats [--json] [--db 路径]
              devices [--json] [--db 路径]
              config                                   查看配置
              config set <key> <value>                 修改配置并保存
                      （RecvDirectory / AutoCopyClipboard / CompressionEnabled /
                        EncryptionEnabled / RfcommChunkSize）

            测试
              selftest                                 无蓝牙硬件自检：
                        分帧/CRC、ECDH+AES-GCM 加密、Deflate 压缩、存储与导出
              repl                                     交互式会话（推荐用于模拟真实交互）

            示例：
              btcli serve --name TestPC
              btcli scan --seconds 5
              btcli connect A0E9F1D27B3C --peer TestPC
              btcli send-file A0E9F1D27B3C C:\tmp\doc.pdf --compress --encrypt
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
