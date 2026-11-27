using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

public class ObexClientTests
{
    private static byte[] ConnectOkResponse() => new ObexPacket
    {
        Opcode = ObexResponseCode.Ok,
        IncludeConnectFields = true,
        Version = 0x10,
        Flags = 0x00,
        MaxPacketLength = ushort.MaxValue
    }.ToBytes();

    private static byte[] SimpleResponse(byte opcode) => new ObexPacket { Opcode = opcode }.ToBytes();

    private static byte[] UnauthorizedWithNonceResponse()
    {
        var nonce = Enumerable.Range(0, 16).Select(i => (byte)i).ToArray();
        var challengeValue = new byte[18];
        challengeValue[0] = 0x00;
        challengeValue[1] = 0x10;
        Array.Copy(nonce, 0, challengeValue, 2, 16);

        var packet = new ObexPacket { Opcode = ObexResponseCode.Unauthorized };
        packet.Headers.Add(new ObexHeader(ObexHeaderId.AuthChallenge, challengeValue));
        return packet.ToBytes();
    }

    [Fact]
    public async Task Connect_Success()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);

        await client.ConnectAsync();

        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal((byte)ObexRequestOpcode.Connect, packets![0].Opcode);
    }

    [Fact]
    public async Task Connect_Unauthorized_WithoutPassword_Throws()
    {
        var transport = new MemoryObexTransport(UnauthorizedWithNonceResponse());
        await using var client = new ObexClient(transport, authPassword: "");

        var ex = await Assert.ThrowsAsync<ObexException>(() => client.ConnectAsync());
        Assert.Contains("认证", ex.Message);
    }

    [Fact]
    public async Task Connect_Unauthorized_WithPassword_RetriesAndSucceeds()
    {
        var transport = new MemoryObexTransport(
            UnauthorizedWithNonceResponse(),
            ConnectOkResponse());
        await using var client = new ObexClient(transport, authPassword: "secret");

        await client.ConnectAsync();

        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal(2, packets!.Count);
        var second = packets[1];
        Assert.Contains(second.Headers, h => h.Id == ObexHeaderId.AuthResponse);
    }

    [Fact]
    public async Task Connect_NonOkResponse_Throws()
    {
        var transport = new MemoryObexTransport(SimpleResponse(ObexResponseCode.ServiceUnavailable));
        await using var client = new ObexClient(transport);

        var ex = await Assert.ThrowsAsync<ObexException>(() => client.ConnectAsync());
        Assert.Contains("CONNECT 失败", ex.Message);
    }

    [Fact]
    public async Task Connect_0xA0SuccessWithConnectionId_Succeeds()
    {
        // 蓝牙 OPP 规范：CONNECT 成功响应码为 0xA0（Success Final），且带固定字段与 ConnectionId
        var connectOk = new ObexPacket
        {
            Opcode = ObexResponseCode.SuccessFinal,
            IncludeConnectFields = true,
            Version = 0x10,
            Flags = 0x00,
            MaxPacketLength = 0xFFFE
        };
        connectOk.Headers.Add(ObexHeader.ConnectionId(1));
        var transport = new MemoryObexTransport(connectOk.ToBytes());
        await using var client = new ObexClient(transport);

        await client.ConnectAsync();

        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal((byte)ObexRequestOpcode.Connect, packets![0].Opcode);
    }

    [Fact]
    public async Task Push_SinglePacket_WhenDataFits()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[100];
        using var ms = new MemoryStream(data);
        var result = await client.PushAsync("small.bin", "application/octet-stream", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, 32768);

        Assert.Equal(ObexResponseCode.Ok, result.ResponseCode);
        Assert.Equal(data.Length, result.BytesSent);
        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal((byte)ObexRequestOpcode.Connect, packets![0].Opcode);
        // 两阶段 PUT：packets[1] 为仅含头的 PUT（无 Body），packets[2] 为 PutFinal + EndOfBody
        Assert.Equal((byte)ObexRequestOpcode.Put, packets[1].Opcode);
        Assert.DoesNotContain(packets[1].Headers, h => h.Id == ObexHeaderId.Body);
        Assert.DoesNotContain(packets[1].Headers, h => h.Id == ObexHeaderId.EndOfBody);
        Assert.Contains(packets[1].Headers, h => h.Id == ObexHeaderId.Name);
        Assert.Contains(packets[1].Headers, h => h.Id == ObexHeaderId.Length);
        Assert.Equal((byte)ObexRequestOpcode.PutFinal, packets[2].Opcode);
        Assert.Contains(packets[2].Headers, h => h.Id == ObexHeaderId.EndOfBody);
    }

    [Fact]
    public async Task Push_NameHeader_NoBom_Utf16BeNullTerminated()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[16];
        using var ms = new MemoryStream(data);
        await client.PushAsync("你好.txt", "application/octet-stream", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, 32768);

        var packets = transport.ParseWrittenPackets();
        var nameHeader = packets![1].Headers.First(h => h.Id == ObexHeaderId.Name);
        // 无 BOM：UTF-16BE（0x4F60 0x597D 0x002E 0x0074 0x0078 0x0074）+ 0x0000 结尾
        Assert.False(nameHeader.Value[0] == 0xFE && nameHeader.Value[1] == 0xFF);
        Assert.Equal(0x4F, nameHeader.Value[0]);
        Assert.Equal(0x60, nameHeader.Value[1]);
        Assert.Equal(0x00, nameHeader.Value[^2]);
        Assert.Equal(0x00, nameHeader.Value[^1]);
        Assert.Equal(14, nameHeader.Value.Length);
    }

    [Fact]
    public async Task Push_NameHeader_WithBomOption()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport, new ObexOptions { NameUseBom = true });
        await client.ConnectAsync();

        var data = new byte[16];
        using var ms = new MemoryStream(data);
        await client.PushAsync("a.bin", "application/octet-stream", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, 32768);

        var packets = transport.ParseWrittenPackets();
        var nameHeader = packets![1].Headers.First(h => h.Id == ObexHeaderId.Name);
        Assert.Equal(0xFE, nameHeader.Value[0]);
        Assert.Equal(0xFF, nameHeader.Value[1]);
        Assert.Equal(0x00, nameHeader.Value[^1]);
    }

    [Fact]
    public async Task Push_MultipleChunks_AtExactBoundary()
    {
        const int chunk = 32768;
        var data = new byte[chunk * 2];
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        using var ms = new MemoryStream(data);
        var result = await client.PushAsync("big.bin", "application/octet-stream", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, chunk);

        Assert.Equal(data.Length, result.BytesSent);
        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        // CONNECT + 头包 PUT(0x02，无 Body) + Body PUT(0x02) + PUT 最终(0x82)
        Assert.Equal((byte)ObexRequestOpcode.Put, packets![1].Opcode);
        Assert.DoesNotContain(packets[1].Headers, h => h.Id == ObexHeaderId.Body);
        Assert.Equal((byte)ObexRequestOpcode.Put, packets[2].Opcode);
        Assert.Equal((byte)ObexRequestOpcode.PutFinal, packets[3].Opcode);
        Assert.Equal(32768, packets[2].Headers.First(h => h.Id == ObexHeaderId.Body).Value.Length);
        Assert.Equal(32768, packets[3].Headers.First(h => h.Id == ObexHeaderId.EndOfBody).Value.Length);
    }

    [Fact]
    public async Task Push_EmptyFile_SendsHeadersThenEmptyEndOfBody()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var result = await client.PushAsync("empty.bin", "application/octet-stream", 0,
            (_, _, _, _) => Task.FromResult(0), 32768);

        Assert.Equal(0, result.BytesSent);
        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal((byte)ObexRequestOpcode.Put, packets![1].Opcode);
        Assert.DoesNotContain(packets[1].Headers, h => h.Id == ObexHeaderId.Body);
        Assert.Equal((byte)ObexRequestOpcode.PutFinal, packets[2].Opcode);
        var eob = packets[2].Headers.First(h => h.Id == ObexHeaderId.EndOfBody);
        Assert.Empty(eob.Value);
    }

    [Fact]
    public async Task Push_ServerRejectsHeaderPut_Throws()
    {
        var transport = new MemoryObexTransport(
            ConnectOkResponse(),
            SimpleResponse(ObexResponseCode.Forbidden));
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[100];
        using var ms = new MemoryStream(data);
        var ex = await Assert.ThrowsAsync<ObexException>(() =>
            client.PushAsync("a.bin", "", data.Length,
                (offset, buffer, count, _) =>
                {
                    ms.Seek(offset, SeekOrigin.Begin);
                    var n = ms.Read(buffer, 0, count);
                    return Task.FromResult(n);
                }, 32768));

        Assert.Contains("头包被拒绝", ex.Message);
    }

    [Fact]
    public async Task Push_ServerAcceptsWithoutBody_Succeeds()
    {
        // 个别服务端对头包直接返回成功（空对象等场景），按成功处理且不再发送 Body
        var transport = new MemoryObexTransport(
            ConnectOkResponse(),
            SimpleResponse(ObexResponseCode.Ok));
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[100];
        using var ms = new MemoryStream(data);
        var result = await client.PushAsync("a.bin", "", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, 32768);

        Assert.Equal(ObexResponseCode.Ok, result.ResponseCode);
        Assert.Equal(0, result.BytesSent);
        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal(2, packets!.Count); // CONNECT + 头包 PUT
    }

    [Fact]
    public async Task Push_ServerErrorResponse_ThrowsWithCode()
    {
        var transport = new MemoryObexTransport(
            ConnectOkResponse(),
            SimpleResponse(0xC0));
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[1000];
        using var ms = new MemoryStream(data);
        var ex = await Assert.ThrowsAsync<ObexException>(() =>
            client.PushAsync("a.bin", "", data.Length,
                (offset, buffer, count, _) =>
                {
                    ms.Seek(offset, SeekOrigin.Begin);
                    var n = ms.Read(buffer, 0, count);
                    return Task.FromResult(n);
                }, 32768));

        Assert.Contains("0xC0", ex.Message);
    }

    [Fact]
    public async Task Push_FinalResponse0xA0_Succeeds()
    {
        var transport = new MemoryObexTransport(
            ConnectOkResponse(),
            SimpleResponse(ObexResponseCode.Continue),
            SimpleResponse(ObexResponseCode.SuccessFinal));
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[100];
        using var ms = new MemoryStream(data);
        var result = await client.PushAsync("a.bin", "", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, 32768);

        Assert.Equal(ObexResponseCode.SuccessFinal, result.ResponseCode);
        Assert.Equal(data.Length, result.BytesSent);
    }

    [Fact]
    public async Task Push_EarlyOkResponse_StopsSending()
    {
        var transport = new MemoryObexTransport(
            ConnectOkResponse(),
            SimpleResponse(ObexResponseCode.Continue),
            SimpleResponse(ObexResponseCode.Ok));
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        var data = new byte[100_000];
        using var ms = new MemoryStream(data);
        var result = await client.PushAsync("a.bin", "", data.Length,
            (offset, buffer, count, _) =>
            {
                ms.Seek(offset, SeekOrigin.Begin);
                var n = ms.Read(buffer, 0, count);
                return Task.FromResult(n);
            }, 32768);

        Assert.Equal(ObexResponseCode.Ok, result.ResponseCode);
        Assert.Equal(32768, result.BytesSent);
    }

    [Fact]
    public async Task Push_Cancelled_Throws()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();
        using var cts = new CancellationTokenSource();
        cts.Cancel();

        var data = new byte[1000];
        using var ms = new MemoryStream(data);
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() =>
            client.PushAsync("a.bin", "", data.Length,
                (_, buffer, count, _) => Task.FromResult(Math.Min(count, 100)),
                32768, cts.Token));
    }

    [Fact]
    public async Task Disconnect_Success()
    {
        var transport = new MemoryObexTransport { AutoReply = true };
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        await client.DisconnectAsync();

        var packets = transport.ParseWrittenPackets();
        Assert.NotNull(packets);
        Assert.Equal((byte)ObexRequestOpcode.Disconnect, packets![^1].Opcode);
    }

    [Fact]
    public async Task Disconnect_0xA0Response_IsAccepted()
    {
        var transport = new MemoryObexTransport(
            ConnectOkResponse(),
            SimpleResponse(ObexResponseCode.SuccessFinal));
        await using var client = new ObexClient(transport);
        await client.ConnectAsync();

        // 不应抛出异常（0xA0 视为成功）
        await client.DisconnectAsync();
    }
}
