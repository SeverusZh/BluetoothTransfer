namespace BluetoothTransfer.Services;

/// <summary>
/// 一次设备解析结果（只保留发送通道真正需要的字段，不含任何 WinRT 句柄）。
/// </summary>
public readonly record struct CachedDeviceInfo(string Id, string Addr, string Name, bool IsPaired);

/// <summary>
/// 设备解析结果缓存（纯逻辑，不依赖 WinRT/WPF/蓝牙硬件，可跨平台单测）。
///
/// 背景：发送队列会连续推送多个文件/文本/文件夹，而每次发送前的设备解析（OPP 全量发现 + SDP 枚举，
/// 或助手通道的 OPP 发现 + 已配对设备兜底）都要遍历系统设备列表，属于纯前置开销；
/// 对端只有蓝牙通道时这段开销会明显拉长小文件的首字节延迟。按「规范化 MAC 地址」缓存
/// Id/Name/IsPaired，可在 TTL 内直接复用，跳过重复发现。
///
/// 失效策略（与 <c>BluetoothTransfer.Core.Discovery.AssistantPresenceTracker</c> 的结论缓存思路一致）：
/// 1. 只缓存正结果（成功解析到的设备），且带 TTL（默认 60 秒）：过期即视为未命中并重新解析，
///    对端重启蓝牙/重新配对后不会长期复用失效的 WinRT 设备 Id。
/// 2. 解析失败不缓存任何否定结论；连接/发送失败时由调用方显式 <see cref="Invalidate"/>，
///    下一次发送或重试会重新解析（避免把一次瞬时失败锁死成整个会话的失败）。
/// 3. 地址按 <see cref="NormalizeAddress"/> 规范化后作为键（大小写、冒号/连字符写法不敏感）。
/// </summary>
public sealed class DeviceInfoCache
{
    /// <summary>默认缓存有效期：60 秒（足以覆盖一次多文件发送，又不会长期复用失效 Id）。</summary>
    public static readonly TimeSpan DefaultTtl = TimeSpan.FromSeconds(60);

    private sealed class Entry
    {
        public string Id { get; init; } = "";
        public string Addr { get; init; } = "";
        public string Name { get; init; } = "";
        public bool IsPaired { get; init; }
        public DateTime ResolvedAt { get; init; }
    }

    private readonly Dictionary<string, Entry> _entries = new(StringComparer.OrdinalIgnoreCase);
    private readonly object _gate = new();
    private readonly TimeSpan _ttl;
    private readonly Func<DateTime> _clock;

    /// <param name="ttl">缓存有效期，默认 <see cref="DefaultTtl"/>。</param>
    /// <param name="clock">时间源（返回 UTC 时间；测试注入固定时钟，无需等待真实时间）。</param>
    public DeviceInfoCache(TimeSpan? ttl = null, Func<DateTime>? clock = null)
    {
        _ttl = ttl ?? DefaultTtl;
        _clock = clock ?? (() => DateTime.UtcNow);
    }

    /// <summary>当前缓存有效期。</summary>
    public TimeSpan Ttl => _ttl;

    /// <summary>当前缓存条目数（含已过期但尚未访问淘汰的条目，仅供诊断/测试）。</summary>
    public int Count
    {
        get { lock (_gate) return _entries.Count; }
    }

    /// <summary>
    /// 尝试取未过期的缓存结果；未命中（无记录/已过期/地址为空）返回 false。
    /// 过期条目顺带移除，避免无限增长。
    /// </summary>
    public bool TryGet(string? addr, out CachedDeviceInfo device)
    {
        device = default;
        var key = NormalizeAddress(addr);
        if (key.Length == 0) return false;
        lock (_gate)
        {
            if (!_entries.TryGetValue(key, out var entry)) return false;
            // 与 AssistantPresenceTracker 一致：恰好达到 TTL 边界仍视为有效，超过才过期。
            if (_clock() - entry.ResolvedAt > _ttl)
            {
                _entries.Remove(key);
                return false;
            }
            device = new CachedDeviceInfo(entry.Id, entry.Addr, entry.Name, entry.IsPaired);
            return true;
        }
    }

    /// <summary>
    /// 写入（或刷新）一个解析结果。地址为空时不缓存（无法作为键，避免不同设备串号），
    /// 返回本次写入的值供调用方直接使用。
    /// </summary>
    public CachedDeviceInfo Set(string? addr, string? id, string? name, bool isPaired)
    {
        var info = new CachedDeviceInfo(id ?? "", addr?.Trim() ?? "", name ?? "", isPaired);
        var key = NormalizeAddress(addr);
        if (key.Length == 0) return info;
        lock (_gate)
        {
            _entries[key] = new Entry
            {
                Id = info.Id,
                Addr = info.Addr,
                Name = info.Name,
                IsPaired = info.IsPaired,
                ResolvedAt = _clock()
            };
        }
        return info;
    }

    /// <summary>失效某地址的缓存（解析失败/连接失败后调用，下次发送重新解析）。</summary>
    public void Invalidate(string? addr)
    {
        var key = NormalizeAddress(addr);
        if (key.Length == 0) return;
        lock (_gate) _entries.Remove(key);
    }

    /// <summary>清空全部缓存（例如用户手动重新扫描设备）。</summary>
    public void Clear()
    {
        lock (_gate) _entries.Clear();
    }

    /// <summary>
    /// 蓝牙地址规范化：去掉冒号/连字符、统一小写后按 6 字节重排；非 12 位写法按去首尾空白后原样作为键。
    /// 语义与 <see cref="OppDiscoveryService.NormalizeAddr"/> 保持一致——本类型保持零 WinRT 依赖，
    /// 故内联同一段纯字符串逻辑（两处必须同步修改，测试覆盖两种写法等价）。
    /// </summary>
    internal static string NormalizeAddress(string? addr)
    {
        if (string.IsNullOrWhiteSpace(addr)) return "";
        var clean = addr.Replace(":", "").Replace("-", "").Trim();
        if (clean.Length != 12) return addr.Trim();
        return string.Join(":", Enumerable.Range(0, 6).Select(i => clean.Substring(i * 2, 2))).ToLowerInvariant();
    }
}
