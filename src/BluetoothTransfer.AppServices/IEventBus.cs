namespace BluetoothTransfer.Services;

/// <summary>
/// 事件总线接口：GUI 侧仅使用的泛型 Subscribe/Unsubscribe/Publish 三成员，
/// 供 <see cref="OppViewModel"/> 与单元测试注入 fake 铺路。
/// 实现见 <see cref="EventBus"/>。
/// </summary>
public interface IEventBus
{
    void Subscribe<T>(Action<T> handler);

    void Unsubscribe<T>(Action<T> handler);

    void Publish<T>(T message);
}
