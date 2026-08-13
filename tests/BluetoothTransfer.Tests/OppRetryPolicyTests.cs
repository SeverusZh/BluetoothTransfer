using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class OppRetryPolicyTests
{
    [Fact]
    public void MaxAttempts_RetryCountPlusOne()
    {
        Assert.Equal(1, new OppRetryPolicy(0, 3).MaxAttempts);
        Assert.Equal(4, new OppRetryPolicy(3, 3).MaxAttempts);
        Assert.Equal(1, new OppRetryPolicy(-5, 3).MaxAttempts);
    }

    [Theory]
    [InlineData("OBEX PUT 失败：Forbidden (0xC3)")]
    [InlineData("对端提前关闭连接（响应头不完整）")]
    [InlineData("连接或发送超时")]
    [InlineData("设备未就绪")]
    [InlineData("你的主机中的软件中止了一个已建立的连接")]
    [InlineData("RFCOMM 连接失败")]
    public void ShouldRetry_RetryableErrors_True(string message)
    {
        Assert.True(OppRetryPolicy.IsRetryableError(message));
        Assert.True(new OppRetryPolicy(3, 3).ShouldRetry(1, message));
    }

    [Theory]
    [InlineData("用户取消")]
    [InlineData("设备未配对")]
    [InlineData("设备服务 SDP 未声明 OPP（0x1105），不支持蓝牙文件接收")]
    [InlineData("文件不存在")]
    [InlineData("文件超过 4GB 上限")]
    public void ShouldRetry_NonRetryableErrors_False(string message)
    {
        Assert.False(OppRetryPolicy.IsRetryableError(message));
        Assert.False(new OppRetryPolicy(3, 3).ShouldRetry(1, message));
    }

    [Fact]
    public void ShouldRetry_ExceedsMaxAttempts_False()
    {
        var policy = new OppRetryPolicy(2, 3);
        Assert.True(policy.ShouldRetry(1, "连接失败"));
        Assert.True(policy.ShouldRetry(2, "连接失败"));
        Assert.False(policy.ShouldRetry(3, "连接失败"));
    }

    [Fact]
    public void NextDelay_ExponentialBackoff()
    {
        var policy = new OppRetryPolicy(3, 5);
        Assert.Equal(TimeSpan.FromSeconds(5), policy.NextDelay(1));
        Assert.Equal(TimeSpan.FromSeconds(10), policy.NextDelay(2));
        Assert.Equal(TimeSpan.FromSeconds(20), policy.NextDelay(3));
        Assert.Equal(TimeSpan.FromSeconds(40), policy.NextDelay(4));
    }

    [Fact]
    public void NextDelay_DefaultBase_ThreeSixTwelve()
    {
        var policy = new OppRetryPolicy(3, 3);
        Assert.Equal(TimeSpan.FromSeconds(3), policy.NextDelay(1));
        Assert.Equal(TimeSpan.FromSeconds(6), policy.NextDelay(2));
        Assert.Equal(TimeSpan.FromSeconds(12), policy.NextDelay(3));
    }

    [Fact]
    public void NullOrEmptyError_Retryable()
    {
        Assert.True(OppRetryPolicy.IsRetryableError(null));
        Assert.True(OppRetryPolicy.IsRetryableError(""));
    }
}
