namespace BluetoothTransfer.Core.Protocol;

/// <summary>私有助手协议常量。v1：明文，无加密。</summary>
public static class AsstConst
{
    /// <summary>自定义 RFCOMM 服务 UUID（发送端 SDP 探测依据）。</summary>
    public static readonly Guid ServiceUuid = new("9F3C0E2A-1B4D-4E6F-8A9B-0C1D2E3F4A5B");

    /// <summary>帧魔数 'A'。</summary>
    public const byte Magic = 0x41;

    /// <summary>协议版本。</summary>
    public const byte Version = 1;

    /// <summary>默认分片大小（字节），与 OPP 的 OppChunkSize 默认值一致。</summary>
    public const int DefaultChunkSize = 32768;

    /// <summary>单帧上限（含头）。</summary>
    public const int MaxFrameSize = 1024 * 1024;
}
