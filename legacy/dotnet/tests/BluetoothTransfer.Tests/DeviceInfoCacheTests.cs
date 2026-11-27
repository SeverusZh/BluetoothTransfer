using BluetoothTransfer.Services;
using Xunit;

namespace BluetoothTransfer.Tests;

/// <summary>
/// <see cref="DeviceInfoCache"/> 单元测试：纯逻辑 + 注入时钟，不依赖 WinRT/蓝牙硬件（Linux harness 可运行）。
/// 覆盖：TTL 命中/过期边界、地址规范化、失效/清空、覆盖刷新、空地址不缓存、并发访问。
/// </summary>
public class DeviceInfoCacheTests
{
    private const string Addr = "AA:BB:CC:DD:EE:FF";

    /// <summary>
    /// 构造注入时钟的缓存。注意必须传入 <see cref="Func{DateTime}"/> 而不是 DateTime 值：
    /// 传值会让闭包捕获参数拷贝，测试随后修改局部变量时注入时钟不会前进（TTL 边界用例曾因此假通过/误失败）。
    /// </summary>
    private static DeviceInfoCache NewCache(Func<DateTime> clock, TimeSpan? ttl = null)
        => new(ttl ?? TimeSpan.FromSeconds(60), clock);

    [Fact]
    public void DefaultTtl_IsSixtySeconds()
    {
        Assert.Equal(TimeSpan.FromSeconds(60), DeviceInfoCache.DefaultTtl);
        Assert.Equal(DeviceInfoCache.DefaultTtl, new DeviceInfoCache().Ttl);
    }

    [Fact]
    public void TryGet_WithoutEntry_ReturnsFalse()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);

        Assert.False(cache.TryGet(Addr, out var device));
        Assert.Equal(default(CachedDeviceInfo), device);
        Assert.Equal(0, cache.Count);
    }

    [Fact]
    public void SetThenTryGet_ReturnsCachedFields()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);

        var stored = cache.Set(Addr, "dev-id-1", "我的手机", isPaired: true);
        var hit = cache.TryGet(Addr, out var device);

        Assert.True(hit);
        Assert.Equal(new CachedDeviceInfo("dev-id-1", Addr, "我的手机", true), device);
        Assert.Equal(stored, device);
    }

    [Fact]
    public void TryGet_AtExactTtlBoundary_StillHits_ThenExpires()
    {
        var now = DateTime.UnixEpoch;
        var cache = NewCache(() => now);

        cache.Set(Addr, "id", "手机", true);

        now = now.AddSeconds(60); // 恰好达到 TTL：仍有效（与 AssistantPresenceTracker 边界语义一致）
        Assert.True(cache.TryGet(Addr, out _));

        now = now.AddMilliseconds(1); // 超过 TTL：过期并淘汰
        Assert.False(cache.TryGet(Addr, out _));
        Assert.Equal(0, cache.Count);
    }

    [Fact]
    public void Address_Normalization_IsFormatInsensitive()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);

        cache.Set("aa-bb-cc-dd-ee-ff", "id", "手机", true);

        // 冒号/连字符/大小写写法不同，仍命中同一条缓存。
        Assert.True(cache.TryGet("AA:BB:CC:DD:EE:FF", out var device));
        Assert.Equal("id", device.Id);
        Assert.Equal(1, cache.Count);
    }

    [Theory]
    [InlineData("AA:BB:CC:DD:EE:FF")]
    [InlineData("aa-bb-cc-dd-ee-ff")]
    [InlineData("AABBCCDDEEFF")]
    [InlineData("not-an-address")]
    public void NormalizeAddress_StaysConsistentWithOppDiscoveryService(string addr)
    {
        // DeviceInfoCache 为保持零 WinRT 依赖内联了地址规范化逻辑，必须与 OppDiscoveryService 的
        // 生产实现逐字等价，否则缓存键与设备解析结果会对不上（两处需同步修改）。
        Assert.Equal(OppDiscoveryService.NormalizeAddr(addr), DeviceInfoCache.NormalizeAddress(addr));
    }

    [Fact]
    public void Invalidate_RemovesEntry_ForcingReprobe()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);
        cache.Set(Addr, "id", "手机", true);
        Assert.True(cache.TryGet(Addr, out _));

        cache.Invalidate(Addr);

        Assert.False(cache.TryGet(Addr, out _));
        // 失效后重新 Set 可再次命中（对应“解析失败/连接失败 → 重新解析”的流程）。
        cache.Set(Addr, "id-2", "手机", true);
        Assert.True(cache.TryGet(Addr, out var device));
        Assert.Equal("id-2", device.Id);
    }

    [Fact]
    public void Clear_RemovesAllEntries()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);
        cache.Set(Addr, "id", "手机", true);
        cache.Set("11:22:33:44:55:66", "id-2", "平板", true);

        cache.Clear();

        Assert.Equal(0, cache.Count);
        Assert.False(cache.TryGet(Addr, out _));
        Assert.False(cache.TryGet("11:22:33:44:55:66", out _));
    }

    [Fact]
    public void Set_SameAddressTwice_RefreshesTimestampAndFields()
    {
        var now = DateTime.UnixEpoch;
        var cache = NewCache(() => now);
        cache.Set(Addr, "id-old", "旧名字", false);

        now = now.AddSeconds(50);
        cache.Set(Addr, "id-new", "新名字", true);

        // 刷新后从新时间点起算 TTL：再经过 50 秒（距首次写入 100 秒）仍应命中。
        now = now.AddSeconds(50);
        Assert.True(cache.TryGet(Addr, out var device));
        Assert.Equal("id-new", device.Id);
        Assert.Equal("新名字", device.Name);
        Assert.True(device.IsPaired);
        Assert.Equal(1, cache.Count); // 覆盖而非追加
    }

    [Fact]
    public void Set_BlankAddress_IsNotCached()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);

        var stored = cache.Set("  ", "id", "手机", true);

        Assert.Equal("id", stored.Id);
        Assert.Equal(0, cache.Count);
        Assert.False(cache.TryGet("", out _));
        Assert.False(cache.TryGet(null, out _));
    }

    [Fact]
    public void TryGet_NonStandardAddress_IsTreatedAsLiteralKey()
    {
        var cache = NewCache(() => DateTime.UnixEpoch);
        cache.Set("device-1", "id", "自定义", true);

        Assert.True(cache.TryGet("device-1", out var device));
        Assert.Equal("id", device.Id);
        // 非 12 位规范写法不与蓝牙地址互相串号。
        Assert.False(cache.TryGet(Addr, out _));
    }

    [Fact]
    public void ConcurrentSetAndGet_DoesNotThrow()
    {
        var cache = new DeviceInfoCache();

        Parallel.For(0, 200, i =>
        {
            var addr = $"AA:BB:CC:DD:EE:{i % 10:X2}";
            cache.Set(addr, $"id-{i}", "设备", i % 2 == 0);
            cache.TryGet(addr, out _);
            if (i % 7 == 0) cache.Invalidate(addr);
        });

        Assert.True(cache.Count <= 10);
    }
}
