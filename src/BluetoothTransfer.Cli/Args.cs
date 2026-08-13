namespace BluetoothTransfer.Cli;

/// <summary>
/// 极简参数解析：支持位置参数、--flag、--key=value、--key value（仅已知取值选项）。
/// </summary>
public sealed class Args
{
    /// <summary>需要取值的选项集合（形如 --key value 或 --key=value）。</summary>
    private static readonly HashSet<string> ValuedOptions = new(StringComparer.OrdinalIgnoreCase)
    {
        "seconds", "timeout", "name", "peer", "chunk", "limit", "direction", "type",
        "status", "search", "from", "to", "db", "pin", "mode", "root", "sort"
    };

    /// <summary>仅作开关、不带取值的已知标志集合。</summary>
    private static readonly HashSet<string> KnownFlags = new(StringComparer.OrdinalIgnoreCase)
    {
        "json", "paired-only", "zip", "verbose", "yes", "unset"
    };

    private readonly List<string> _positional = new();
    private readonly Dictionary<string, string> _options = new(StringComparer.OrdinalIgnoreCase);
    private readonly HashSet<string> _flags = new(StringComparer.OrdinalIgnoreCase);
    private readonly List<string> _unknownOptions = new();

    /// <summary>首次出现的参数校验错误（如带值选项缺值）；无错误为 null。</summary>
    public string? Error { get; private set; }

    /// <summary>解析时识别出的未知选项（不含值、未匹配已知选项集）。</summary>
    public IReadOnlyList<string> UnknownOptions => _unknownOptions;

    public static Args Parse(IEnumerable<string> args)
    {
        var parsed = new Args();
        var tokens = args.ToList();
        for (var i = 0; i < tokens.Count; i++)
        {
            var token = tokens[i];
            if (token.StartsWith("--", StringComparison.Ordinal))
            {
                var body = token[2..];
                var eq = body.IndexOf('=');
                if (eq >= 0)
                {
                    // --key=value 形式：仅当 key 已知时记录，否则视为未知选项。
                    var key = body[..eq];
                    if (ValuedOptions.Contains(key) || KnownFlags.Contains(key))
                        parsed._options[key] = body[(eq + 1)..];
                    else
                        parsed._unknownOptions.Add(key);
                }
                else if (ValuedOptions.Contains(body))
                {
                    // 带值选项若后面紧跟另一个选项或参数已耗尽，视为缺值错误（避免吞掉下一个参数）。
                    if (i + 1 < tokens.Count && !tokens[i + 1].StartsWith("--", StringComparison.Ordinal))
                        parsed._options[body] = tokens[++i];
                    else
                        parsed.Error ??= $"选项 --{body} 需要值";
                }
                else if (KnownFlags.Contains(body))
                {
                    parsed._flags.Add(body);
                }
                else
                {
                    parsed._unknownOptions.Add(body);
                }
            }
            else
            {
                parsed._positional.Add(token);
            }
        }
        return parsed;
    }

    public int Count => _positional.Count;

    public string? Get(int index) => index < _positional.Count ? _positional[index] : null;

    public string? Option(string name) => _options.TryGetValue(name, out var value) ? value : null;

    public bool Has(string name) => _flags.Contains(name) || _options.ContainsKey(name);

    /// <summary>将第 <paramref name="from"/> 个位置参数起的所有参数重新拼接（用于发送多词文本）。</summary>
    public string RemainingFrom(int from)
        => from >= _positional.Count ? "" : string.Join(' ', _positional.Skip(from));
}
