namespace BluetoothTransfer.Services;

/// <summary>OBEX PUT 推送结果。</summary>
public record ObexPushResult(byte ResponseCode, long BytesSent);

/// <summary>OBEX 客户端行为选项。</summary>
public sealed class ObexOptions
{
    /// <summary>Name 头是否携带 UTF-16 BOM。默认 false（与 32feet/Windows 原生向导一致，Android 兼容）。</summary>
    public bool NameUseBom { get; set; }
}

/// <summary>
/// OBEX 客户端：CONNECT（含认证挑战重试）→ 流式 PUT → DISCONNECT。
/// 仅依赖 <see cref="IObexTransport"/>，可离线测试。
/// </summary>
public sealed class ObexClient : IAsyncDisposable
{
    private readonly IObexTransport _transport;
    private readonly EventBus? _events;
    private readonly string _authPassword;
    private readonly bool _nameUseBom;
    private uint? _connectionId;
    private ushort _maxPacketLength = ushort.MaxValue;

    public ObexClient(IObexTransport transport, EventBus? events = null, string authPassword = "")
        : this(transport, new ObexOptions(), events, authPassword)
    {
    }

    public ObexClient(IObexTransport transport, ObexOptions options, EventBus? events = null, string authPassword = "")
    {
        _transport = transport ?? throw new ArgumentNullException(nameof(transport));
        options ??= new ObexOptions();
        _events = events;
        _authPassword = authPassword ?? "";
        _nameUseBom = options.NameUseBom;
    }

    public async Task ConnectAsync(CancellationToken ct = default)
    {
        await _transport.WriteAsync(CreateConnectPacket(null).ToBytes(), ct);
        var response = await ReadResponseAsync(connectResponse: true, ct);

        if (response.Opcode == ObexResponseCode.Unauthorized)
        {
            var nonce = response.GetAuthChallengeNonce();
            if (nonce == null || _authPassword.Length == 0)
                throw new ObexException("设备要求 OBEX 认证但未提供 Nonce 或密码（可通过配置 OppAuthPassword 提供）");
            var digest = ObexAuth.ComputeDigest(nonce, _authPassword);
            await _transport.WriteAsync(CreateConnectPacket(ObexHeader.AuthResponse(digest, nonce)).ToBytes(), ct);
            response = await ReadResponseAsync(connectResponse: true, ct);
            if (response.Opcode == ObexResponseCode.Unauthorized)
                throw new ObexException("OBEX 认证被拒绝：用户名/密码不正确或设备要求额外交互");
        }

        if (response.Opcode != ObexResponseCode.Ok && response.Opcode != ObexResponseCode.SuccessFinal)
            throw new ObexException($"OBEX CONNECT 失败：{ObexResponseCode.Describe(response.Opcode)}");

        _connectionId = response.GetConnectionId();
        _maxPacketLength = Math.Min(_maxPacketLength, response.MaxPacketLength);
        if (_maxPacketLength < 64)
            throw new ObexException($"对端 OBEX 最大包长过小（{_maxPacketLength}），无法传输");
        _events?.Publish(new LogEvent("INFO", $"OBEX 已连接，最大包长 {_maxPacketLength}"));
    }

    /// <summary>
    /// 流式 PUT（两阶段，与 32feet/Windows 原生发送向导一致）：
    /// 1) 首包 PUT 仅带 Name/Type/Length 头（不含 Body），等待 0x90 Continue；
    /// 2) 随后按分片发送 Body 包（0x02），末包 PUT Final（0x82）+ EndOfBody。
    /// 大量 Android/Windows OPP 服务端会拒绝"头 + Body 同包"的写法并直接断开，故必须分两阶段。
    /// <paramref name="readChunk"/>(offset, buffer, count, ct) 返回实际读取字节数。
    /// </summary>
    public async Task<ObexPushResult> PushAsync(
        string name, string mimeType, long totalLength,
        Func<long, byte[], int, CancellationToken, Task<int>> readChunk,
        int chunkSize, CancellationToken ct = default)
    {
        if (readChunk == null) throw new ArgumentNullException(nameof(readChunk));
        if (chunkSize <= 0) throw new ArgumentOutOfRangeException(nameof(chunkSize));
        if (totalLength < 0) throw new ArgumentOutOfRangeException(nameof(totalLength));
        ct.ThrowIfCancellationRequested();

        // 阶段 1：仅含头的 PUT，不含 Body
        var headerPacket = new ObexPacket { Opcode = (byte)ObexRequestOpcode.Put };
        AddCommonHeaders(headerPacket);
        headerPacket.Headers.Add(ObexHeader.Name(name, _nameUseBom));
        if (!string.IsNullOrEmpty(mimeType))
            headerPacket.Headers.Add(ObexHeader.Type(mimeType));
        headerPacket.Headers.Add(ObexHeader.LengthHeader(totalLength));
        await _transport.WriteAsync(headerPacket.ToBytes(), ct);

        var headerResponse = await ReadResponseAsync(connectResponse: false, ct);
        if (headerResponse.Opcode != ObexResponseCode.Continue)
        {
            if (ObexResponseCode.IsSuccess(headerResponse.Opcode))
            {
                // 个别服务端在头包即返回成功（例如空文件或已提前接受），按成功处理。
                _events?.Publish(new LogEvent("WARN", "对端在头包 PUT 后直接返回成功（未发送 Body），按成功处理"));
                return new ObexPushResult(headerResponse.Opcode, 0);
            }
            throw new ObexException($"OBEX PUT 头包被拒绝：{ObexResponseCode.Describe(headerResponse.Opcode)}");
        }

        if (totalLength == 0)
        {
            // 空文件：直接发送 EndOfBody 空包
            var emptyFinal = new ObexPacket { Opcode = (byte)ObexRequestOpcode.PutFinal };
            AddCommonHeaders(emptyFinal);
            emptyFinal.Headers.Add(ObexHeader.Body(Array.Empty<byte>(), end: true));
            await _transport.WriteAsync(emptyFinal.ToBytes(), ct);
            var finalResponse = await ReadResponseAsync(connectResponse: false, ct);
            if (!ObexResponseCode.IsSuccess(finalResponse.Opcode))
                throw new ObexException($"OBEX PUT 失败：{ObexResponseCode.Describe(finalResponse.Opcode)}");
            return new ObexPushResult(finalResponse.Opcode, 0);
        }

        // 阶段 2：Body 分片。预留头开销，确保整包不超过对端 MaxPacketLength
        const int headerOverhead = 512;
        var maxBody = Math.Max(16, _maxPacketLength - headerOverhead);
        var chunk = Math.Min(chunkSize, maxBody);

        long offset = 0;
        long bytesSent = 0;
        var buffer = new byte[chunk];

        while (offset < totalLength)
        {
            ct.ThrowIfCancellationRequested();
            var read = await readChunk(offset, buffer, buffer.Length, ct);
            if (read < 0) read = 0;
            if (read == 0 && offset < totalLength)
                throw new ObexException("读取源数据失败（read=0，源不可读或已提前结束）");
            var isLast = offset + read >= totalLength;

            var packet = new ObexPacket
            {
                Opcode = isLast ? (byte)ObexRequestOpcode.PutFinal : (byte)ObexRequestOpcode.Put
            };
            AddCommonHeaders(packet);
            packet.Headers.Add(ObexHeader.Body(buffer.AsSpan(0, read).ToArray(), isLast));

            await _transport.WriteAsync(packet.ToBytes(), ct);
            bytesSent += read;
            offset += read;

            var response = await ReadResponseAsync(connectResponse: false, ct);
            if (isLast)
            {
                if (ObexResponseCode.IsSuccess(response.Opcode))
                {
                    return new ObexPushResult(response.Opcode, bytesSent);
                }
                throw new ObexException($"OBEX PUT 失败：{ObexResponseCode.Describe(response.Opcode)}");
            }

            if (response.Opcode == ObexResponseCode.Continue)
                continue;
            if (ObexResponseCode.IsSuccess(response.Opcode))
            {
                _events?.Publish(new LogEvent("WARN", "对端提前返回 OK，停止后续分片"));
                return new ObexPushResult(response.Opcode, bytesSent);
            }
            throw new ObexException($"OBEX PUT 失败：{ObexResponseCode.Describe(response.Opcode)}");
        }

        throw new ObexException("OBEX PUT 未完成（数据循环异常结束）");
    }

    public async Task DisconnectAsync(CancellationToken ct = default)
    {
        var packet = new ObexPacket { Opcode = (byte)ObexRequestOpcode.Disconnect };
        AddCommonHeaders(packet);
        await _transport.WriteAsync(packet.ToBytes(), ct);
        var response = await ReadResponseAsync(connectResponse: false, ct);
        if (response.Opcode != ObexResponseCode.Ok && response.Opcode != ObexResponseCode.SuccessFinal)
            _events?.Publish(new LogEvent("WARN", $"OBEX DISCONNECT 响应异常：{ObexResponseCode.Describe(response.Opcode)}"));
    }

    public ValueTask DisposeAsync() => _transport.DisposeAsync();

    private ObexPacket CreateConnectPacket(ObexHeader? authResponse)
    {
        var packet = new ObexPacket
        {
            Opcode = (byte)ObexRequestOpcode.Connect,
            Version = 0x10,
            Flags = 0x00,
            MaxPacketLength = ushort.MaxValue
        };
        if (authResponse != null)
            packet.Headers.Add(authResponse);
        return packet;
    }

    private void AddCommonHeaders(ObexPacket packet)
    {
        if (_connectionId is { } id)
            packet.Headers.Add(ObexHeader.ConnectionId(id));
    }

    private async Task<ObexPacket> ReadResponseAsync(bool connectResponse, CancellationToken ct)
    {
        var head = new byte[3];
        if (await ReadExactAsync(head, ct) < 3)
            throw new ObexException("对端提前关闭连接（响应头不完整）");
        var length = (head[1] << 8) | head[2];
        if (length < 3)
            throw new ObexException($"非法 OBEX 响应长度：{length}");

        var body = new byte[length - 3];
        if (await ReadExactAsync(body, ct) < body.Length)
            throw new ObexException("OBEX 响应不完整（对端提前关闭连接）");

        var full = new byte[length];
        Array.Copy(head, 0, full, 0, 3);
        Array.Copy(body, 0, full, 3, body.Length);
        // CONNECT 响应是否带 version/flags/maxPacketLength 固定字段在不同厂商栈上约定不一：
        // 先按"成功响应带固定字段"解析，失败再回退到不带固定字段的解析。
        var isConnectOkResponse = connectResponse &&
            (head[0] == ObexResponseCode.Ok || head[0] == ObexResponseCode.SuccessFinal);
        var packet = ObexPacket.Deserialize(full, isConnectOkResponse, out var error);
        if (packet == null && connectResponse)
            packet = ObexPacket.Deserialize(full, !isConnectOkResponse, out error);
        if (packet == null)
            throw new ObexException($"无法解析 OBEX 响应：{error}（原始字节：{Convert.ToHexString(full)}）");
        return packet;
    }

    private async Task<int> ReadExactAsync(byte[] buffer, CancellationToken ct)
    {
        var total = 0;
        while (total < buffer.Length)
        {
            var n = await _transport.ReadAsync(buffer.AsMemory(total, buffer.Length - total), ct);
            if (n == 0) break;
            total += n;
        }
        return total;
    }
}
