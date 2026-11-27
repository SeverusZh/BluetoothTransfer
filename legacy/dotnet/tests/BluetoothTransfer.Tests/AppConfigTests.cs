using System.Text.Json;
using BluetoothTransfer.Models;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AppConfigTests
{
    [Fact]
    public void NewConfig_Defaults()
    {
        var config = new AppConfig();
        Assert.Equal(32768, config.OppChunkSize);
        Assert.Equal(30, config.OppConnectTimeoutSeconds);
        Assert.Equal(300, config.OppSendTimeoutSeconds);
        Assert.Equal(3, config.OppRetryCount);
        Assert.Equal(3, config.OppRetryDelaySeconds);
        Assert.Equal("auto", config.OppProtectionLevel);
        Assert.False(config.OppNameUseBom);
        // 安全加固：默认每次接收前询问，防止已配对设备直接落盘。
        Assert.True(config.ReceiveAsk);
    }

    [Fact]
    public void Deserialize_OldKeys_IgnoredWithoutError()
    {
        // 旧版（1.0/1.1）配置文件含已移除的键；System.Text.Json 默认跳过未知成员，
        // 已知键仍生效，未知键不抛异常。
        var json = JsonSerializer.Serialize(new
        {
            RecvDirectory = "D:\\old\\recv",
            CompressionEnabled = true,
            EncryptionEnabled = false,
            RfcommChunkSize = 2048,
            AutoCopyClipboard = false,
            OppChunkSize = 16384,
            OppRetryCount = 5
        });

        var config = JsonSerializer.Deserialize<AppConfig>(json);

        Assert.NotNull(config);
        Assert.Equal(16384, config!.OppChunkSize);
        Assert.Equal(5, config.OppRetryCount);
        // 旧键被忽略，不残留到新模型
        Assert.Equal(32768, new AppConfig().OppChunkSize);
    }

    [Fact]
    public void RoundTrip_SaveAndLoad_PreservesValues()
    {
        var config = new AppConfig { OppRetryCount = 7, OppRetryDelaySeconds = 9, OppProtectionLevel = "encrypt" };
        var json = JsonSerializer.Serialize(config);

        var restored = JsonSerializer.Deserialize<AppConfig>(json);

        Assert.NotNull(restored);
        Assert.Equal(7, restored!.OppRetryCount);
        Assert.Equal(9, restored.OppRetryDelaySeconds);
        Assert.Equal("encrypt", restored.OppProtectionLevel);
    }
}
