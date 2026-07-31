using Windows.Devices.Bluetooth.Rfcomm;
using Windows.Networking.Sockets;
using Windows.Storage.Streams;

namespace BluetoothTransfer.Services;

/// <summary>
/// 基于 RFCOMM StreamSocket 的 OBEX 传输（OPP 服务）。
/// </summary>
public sealed class SocketObexTransport : IObexTransport
{
    private readonly StreamSocket _socket;
    private readonly DataWriter _writer;
    private readonly DataReader _reader;
    private bool _disposed;

    private SocketObexTransport(StreamSocket socket)
    {
        _socket = socket;
        _writer = new DataWriter(socket.OutputStream);
        _reader = new DataReader(socket.InputStream)
        {
            InputStreamOptions = InputStreamOptions.Partial
        };
    }

    public static async Task<SocketObexTransport> ConnectAsync(RfcommDeviceService service, CancellationToken ct)
    {
        if (service == null) throw new ArgumentNullException(nameof(service));
        var socket = new StreamSocket();
        try
        {
            await socket.ConnectAsync(service.ConnectionHostName, service.ConnectionServiceName).AsTask(ct);
        }
        catch
        {
            socket.Dispose();
            throw;
        }
        return new SocketObexTransport(socket);
    }

    public async Task WriteAsync(ReadOnlyMemory<byte> buffer, CancellationToken ct = default)
    {
        if (_disposed) throw new ObjectDisposedException(nameof(SocketObexTransport));
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
