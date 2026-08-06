namespace BluetoothTransfer.Core.Protocol;

/// <summary>助手协议消息类型（Task 1 先定义枚举，Task 2 补充载荷编解码）。</summary>
public enum AsstMessageType : byte
{
    Hello = 1,
    Offer = 2,
    Data = 3,
    Ack = 4,
    Done = 5,
    Cancel = 6,
    Error = 7
}
