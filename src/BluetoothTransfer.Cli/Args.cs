namespace BluetoothTransfer.Cli;

/// <summary>
/// 极简参数解析：支持位置参数、--flag、--key=value、--key value（仅已知取值选项）。
/// </summary>
public sealed class Args
{
    private static readonly HashSet<string> ValuedOptions = new(StringComparer.OrdinalIgnoreCase)
    {
        "seconds", "timeout", "name", "peer", "chunk", "limit", "direction", "type",
        "status", "search", "from", "to", "db"
    };

    private readonly List<string> _positional = new();
    private readonly Dictionary<string, string> _options = new(StringComparer.OrdinalIgnoreCase);
    private readonly HashSet<string> _flags = new(StringComparer.OrdinalIgnoreCase);

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
                    parsed._options[body[..eq]] = body[(eq + 1)..];
                }
                else if (ValuedOptions.Contains(body) && i + 1 < tokens.Count)
                {
                    parsed._options[body] = tokens[++i];
                }
                else
                {
                    parsed._flags.Add(body);
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
