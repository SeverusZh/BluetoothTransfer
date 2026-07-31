using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class OppDiscoveryServiceTests
{
    [Theory]
    [InlineData(
        "Bluetooth#Bluetootha8:e2:91:97:b5:39-e8:f7:91:2e:cc:36",
        "e8:f7:91:2e:cc:36")]
    [InlineData(
        "Bluetooth#Bluetootha8:e2:91:97:b5:39-e8:f7:91:2e:cc:36#RFCOMM:00000000:{00001105-0000-1000-8000-00805f9b34fb}",
        "e8:f7:91:2e:cc:36")]
    [InlineData("", "")]
    [InlineData("Bluetooth#Bluetootha8:e2:91:97:b5:39", "")]
    public void ParseMacFromId_ExtractsRemoteMac(string id, string expected)
    {
        Assert.Equal(expected, OppDiscoveryService.ParseMacFromId(id));
    }

    [Theory]
    [InlineData(
        "Bluetooth#Bluetootha8:e2:91:97:b5:39-e8:f7:91:2e:cc:36#RFCOMM:00000000:{00001105-0000-1000-8000-00805f9b34fb}",
        "Bluetooth#Bluetootha8:e2:91:97:b5:39-e8:f7:91:2e:cc:36")]
    [InlineData("Bluetooth#Bluetootha8:e2:91:97:b5:39-e8:f7:91:2e:cc:36",
        "Bluetooth#Bluetootha8:e2:91:97:b5:39-e8:f7:91:2e:cc:36")]
    public void GetBluetoothDeviceId_StripsRfcommSuffix(string serviceId, string expected)
    {
        Assert.Equal(expected, OppDiscoveryService.GetBluetoothDeviceId(serviceId));
    }

    [Theory]
    [InlineData("E8F7912ECC36", "e8:f7:91:2e:cc:36")]
    [InlineData("e8:f7:91:2e:cc:36", "e8:f7:91:2e:cc:36")]
    [InlineData("e8-f7-91-2e-cc-36", "e8:f7:91:2e:cc:36")]
    public void NormalizeAddr_FormatsMac(string input, string expected)
    {
        Assert.Equal(expected, OppDiscoveryService.NormalizeAddr(input));
    }

    [Theory]
    [InlineData("E8F7912ECC36", "e8:f7:91:2e:cc:36", true)]
    [InlineData("e8:f7:91:2e:cc:36", "e8-f7-91-2e-cc-36", true)]
    [InlineData("e8:f7:91:2e:cc:36", "00:11:22:33:44:55", false)]
    public void AddrEquals_IgnoresFormatting(string a, string b, bool expected)
    {
        Assert.Equal(expected, OppDiscoveryService.AddrEquals(a, b));
    }
}
