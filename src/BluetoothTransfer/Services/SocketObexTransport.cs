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

    /// <summary>实际使用的套接字保护级别（用于日志/诊断）。</summary>
    public SocketProtectionLevel ProtectionLevel { get; }

    private SocketObexTransport(StreamSocket socket, SocketProtectionLevel protectionLevel)
    {
        _socket = socket;
        ProtectionLevel = protectionLevel;
        _writer = new DataWriter(socket.OutputStream);
        _reader = new DataReader(socket.InputStream)
        {
            InputStreamOptions = InputStreamOptions.Partial
        };
    }

    public static async Task<SocketObexTransport> ConnectAsync(RfcommDeviceService service, CancellationToken ct)
        => await ConnectAsync(service, "auto", ct);

    /// <summary>
    /// 建立 RFCOMM 连接。<paramref name="protectionLevel"/> 取值 auto/plain/encrypt：
    /// auto 时按服务端 SDP 要求的保护级别连接（服务未要求加密则保持 PlainSocket，
    /// 与微软官方 "RFCOMM Scenario: Send File as a Client" 及 Android 真机验证路径一致）。
    /// </summary>
    public static async Task<SocketObexTransport> ConnectAsync(
        RfcommDeviceService service, string protectionLevel, CancellationToken ct)
    {
        if (service == null) throw new ArgumentNullException(nameof(service));
        var level = ResolveProtectionLevel(service, protectionLevel);
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
        return new SocketObexTransport(socket, level);
    }

    private static SocketProtectionLevel ResolveProtectionLevel(RfcommDeviceService service, string config)
    {
        switch (config?.Trim().ToLowerInvariant())
        {
            case "plain":
                return SocketProtectionLevel.PlainSocket;
            case "encrypt":
                return SocketProtectionLevel.BluetoothEncryptionWithAuthentication;
            default: // auto
                return service.ProtectionLevel != SocketProtectionLevel.PlainSocket
                    ? service.ProtectionLevel
                    : SocketProtectionLevel.PlainSocket;
        }
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
