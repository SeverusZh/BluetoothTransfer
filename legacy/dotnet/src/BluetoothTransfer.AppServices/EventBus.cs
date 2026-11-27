namespace BluetoothTransfer.Services;

public class EventBus : IEventBus
{
    private readonly Dictionary<Type, List<Delegate>> _handlers = new();
    private readonly object _lock = new();

    public void Subscribe<T>(Action<T> handler)
    {
        lock (_lock)
        {
            if (!_handlers.ContainsKey(typeof(T)))
                _handlers[typeof(T)] = new List<Delegate>();
            _handlers[typeof(T)].Add(handler);
        }
    }

    public void Unsubscribe<T>(Action<T> handler)
    {
        lock (_lock)
        {
            if (_handlers.ContainsKey(typeof(T)))
                _handlers[typeof(T)].Remove(handler);
        }
    }

    public void Publish<T>(T message)
    {
        List<Delegate>? handlers;
        lock (_lock)
        {
            if (!_handlers.TryGetValue(typeof(T), out handlers))
                return;
            handlers = new List<Delegate>(handlers);
        }
        foreach (var h in handlers)
            ((Action<T>)h)(message);
    }
}

public record TransferProgressEvent(string TaskId, long BytesSent, long TotalBytes, double Speed);
public record LogEvent(string Level, string Message);
