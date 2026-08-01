using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class OppSdpScanTests
{
    [Fact]
    public void ScanServiceClassList_OppUuid16_Found()
    {
        // DE 序列头(35 03) + UUID16 元素(19 11 05)
        var payload = new byte[] { 0x35, 0x03, 0x19, 0x11, 0x05 };
        var (found, saw) = OppPushService.ScanServiceClassList(payload);
        Assert.True(found);
        Assert.True(saw);
    }

    [Fact]
    public void ScanServiceClassList_OtherUuid_NotOpp()
    {
        // 仅包含 SerialPort UUID16 0x1101
        var payload = new byte[] { 0x35, 0x03, 0x19, 0x11, 0x01 };
        var (found, saw) = OppPushService.ScanServiceClassList(payload);
        Assert.False(found);
        Assert.True(saw);
    }

    [Fact]
    public void ScanServiceClassList_OppUuid128_Found()
    {
        var uuid128 = new byte[]
        {
            0x00, 0x00, 0x11, 0x05, 0x00, 0x00, 0x10, 0x00,
            0x80, 0x00, 0x00, 0x80, 0x5F, 0x9B, 0x34, 0xFB
        };
        var payload = new byte[3 + uuid128.Length];
        payload[0] = 0x35;
        payload[1] = (byte)(0x11 + uuid128.Length);
        payload[2] = 0x1C;
        Array.Copy(uuid128, 0, payload, 3, uuid128.Length);
        var (found, saw) = OppPushService.ScanServiceClassList(payload);
        Assert.True(found);
        Assert.True(saw);
    }

    [Fact]
    public void ScanServiceClassList_Garbage_NoUuidElements()
    {
        var payload = new byte[] { 0x00, 0xFF, 0x12, 0x34, 0x56 };
        var (found, saw) = OppPushService.ScanServiceClassList(payload);
        Assert.False(found);
        Assert.False(saw);
    }

    [Fact]
    public void ScanServiceClassList_Empty_NoUuidElements()
    {
        var (found, saw) = OppPushService.ScanServiceClassList(Array.Empty<byte>());
        Assert.False(found);
        Assert.False(saw);
    }
}
