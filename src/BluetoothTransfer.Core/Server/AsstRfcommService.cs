using System.Threading.Channels;
using BluetoothTransfer.Core.IO;
using BluetoothTransfer.Core.Protocol;
using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Networking.Sockets;

namespace BluetoothTransfer.Core.Server;

/// <summary>
/// 真实 RFCOMM 服务端：注册自定义 UUID 并广播，接受连接后返回 SocketAsstTransport。
/// 服务随进程生命周期存在，无需安装。
/// 注意：本 SDK 投影（10.0.19041）的 StreamSocketListener 无 AcceptAsync，
/// 通过 ConnectionReceived 事件把新连接写入内部队列，再交给 AcceptAsync 消费。
/// </summary>
public sealed class AsstRfcommService : IAsstListener
{
    private readonly RfcommServiceProvider _provider;
    private readonly StreamSocketListener _listener;
    private readonly Channel<IAsstTransport> _incoming = Channel.CreateUnbounded<IAsstTransport>();

    private AsstRfcommService(RfcommServiceProvider provider, StreamSocketListener listener)
    {
        _provider = provider;
        _listener = listener;
        _listener.ConnectionReceived += (_, args) =>
            _incoming.Writer.TryWrite(new SocketAsstTransport(args.Socket));
    }

    public static async Task<AsstRfcommService> StartAsync(
        SocketProtectionLevel level = SocketProtectionLevel.PlainSocket)
    {
        var provider = await RfcommServiceProvider.CreateAsync(RfcommServiceId.FromUuid(AsstConst.ServiceUuid));
        var listener = new StreamSocketListener();
        await listener.BindServiceNameAsync(provider.ServiceId.AsString(), level);
        provider.StartAdvertising(listener, true);
        return new AsstRfcommService(provider, listener);
    }

    public async Task<IAsstTransport> AcceptAsync(CancellationToken ct = default)
    {
        if (!await _incoming.Reader.WaitToReadAsync(ct))
            throw new OperationCanceledException(ct);
        return await _incoming.Reader.ReadAsync(ct);
    }

    public ValueTask DisposeAsync()
    {
        try
        {
            _provider.StopAdvertising();
        }
        catch
        {
            // 停止广播失败忽略
        }
        _listener.Dispose();
        return ValueTask.CompletedTask;
    }
}
