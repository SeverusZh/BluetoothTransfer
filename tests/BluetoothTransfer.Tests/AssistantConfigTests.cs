using BluetoothTransfer.Models;
using Xunit;

namespace BluetoothTransfer.Tests;

public class AssistantConfigTests
{
    [Fact]
    public void TransferMode_DefaultsToAuto()
    {
        Assert.Equal("auto", new AppConfig().TransferMode);
    }

    [Fact]
    public void ChannelDisplay_MapsAssistant()
    {
        Assert.Equal("助手", new TransferRecord { Channel = TransferConst.ChannelAssistant }.ChannelDisplay);
        Assert.Equal("OPP", new TransferRecord { Channel = TransferConst.ChannelOpp }.ChannelDisplay);
    }
}
