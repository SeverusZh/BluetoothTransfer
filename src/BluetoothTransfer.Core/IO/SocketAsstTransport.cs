using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Networking.Sockets;
using Windows.Storage.Streams;

namespace BluetoothTransfer.Core.IO;

/// <summary>基于 RFCOMM StreamSocket 的助手协议传输（客户端连接 / 服务端接受侧）。</summary>
public sealed class SocketAsstTransport : IAsstTransport
{
    private readonly StreamSocket _socket;
    private readonly DataWriter _writer;
    private readonly DataReader _reader;
    private bool _disposed;

    public SocketAsstTransport(StreamSocket socket)
    {
        _socket = socket ?? throw new ArgumentNullException(nameof(socket));
        _writer = new DataWriter(socket.OutputStream);
        _reader = new DataReader(socket.InputStream)
        {
            InputStreamOptions = InputStreamOptions.Partial
        };
    }

    public static async Task<SocketAsstTransport> ConnectAsync(
        RfcommDeviceService service, SocketProtectionLevel level, CancellationToken ct)
    {
        if (service == null) throw new ArgumentNullException(nameof(service));
        var socket = new StreamSocket();
        try
        {
            await socket.ConnectAsync(service.ConnectionHostName, service.ConnectionServiceName, level).AsTask(ct);
        }
        catch
        {
            socket.Dispose();
            throw;
        }
        return new SocketAsstTransport(socket);
    }

    public async Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(SocketAsstTransport));
        _writer.WriteBytes(buffer.ToArray());
        await _writer.StoreAsync().AsTask(ct);
    }

    public async Task<int> ReadAsync(Memory<byte> buffer, CancellationToken ct = default)
    {
        if (_disposed) return 0;
        var loaded = await _reader.LoadAsync((uint)buffer.Length).AsTask(ct);
        if (loaded == 0) return 0;
        var tmp = new byte[loaded];
        _reader.ReadBytes(tmp);
        tmp.CopyTo(buffer);
        return (int)loaded;
    }

    public ValueTask DisposeAsync()
    {
        if (_disposed) return ValueTask.CompletedTask;
        _disposed = true;
        try
        {
            _writer.DetachStream();
            _reader.DetachStream();
        }
        catch
        {
            // 忽略 Detach 清理异常
        }
        _socket.Dispose();
        return ValueTask.CompletedTask;
    }
}
