using System.Collections.Concurrent;
using System.Globalization;
using System.Text;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// <see cref="ILocalizer"/> over the embedded Strings/strings.json: the design handoff's catalog
/// (same keys, same <c>{"zh": {…}, "en": {…}}</c> shape) plus the <c>Error_*</c> texts and a few
/// keys the design lacks. It is the single source for every text of the Windows and Linux apps,
/// static labels included. Any zh-* UI culture gets zh, everything else en; a key missing from
/// zh falls back to en, and one missing from both is returned as the key itself and logged once.
/// </summary>
public sealed class JsonLocalizer : ILocalizer
{
    public const string Chinese = "zh";
    public const string English = "en";

    readonly IReadOnlyDictionary<string, string> _primary;
    readonly IReadOnlyDictionary<string, string> _fallback;
    readonly IAppLog? _log;
    readonly ConcurrentDictionary<string, bool> _reported = new();

    /// <param name="culture">Defaults to <see cref="CultureInfo.CurrentUICulture"/>.</param>
    public JsonLocalizer(IAppLog? log = null, CultureInfo? culture = null)
    {
        _log = log;
        Language = LanguageFor(culture ?? CultureInfo.CurrentUICulture);
        _fallback = Catalog(English);
        _primary = Language == English ? _fallback : Catalog(Language);
    }

    /// <summary>The catalog in use: <see cref="Chinese"/> or <see cref="English"/>.</summary>
    public string Language { get; }

    public string Get(string key)
    {
        if (_primary.TryGetValue(key, out var text) || _fallback.TryGetValue(key, out text)) return text;
        if (_reported.TryAdd(key, true)) _log?.Warn($"missing localized string: {key}");
        return key;
    }

    /// <summary>The text of <paramref name="key"/> with its named placeholders filled.</summary>
    public string Format(string key, params (string Name, object? Value)[] args) => Placeholders.Fill(Get(key), args);

    public static string LanguageFor(CultureInfo culture) =>
        culture.Name.StartsWith("zh", StringComparison.OrdinalIgnoreCase) ? Chinese : English;

    static readonly Lazy<IReadOnlyDictionary<string, IReadOnlyDictionary<string, string>>> Catalogs = new(Load);

    /// <summary>One language of the embedded catalog.</summary>
    public static IReadOnlyDictionary<string, string> Catalog(string language) =>
        Catalogs.Value.TryGetValue(language, out var catalog)
            ? catalog
            : throw new ArgumentException($"no string catalog for {language}", nameof(language));

    static IReadOnlyDictionary<string, IReadOnlyDictionary<string, string>> Load()
    {
        const string name = "PPVPN.App.Core.Strings.strings.json";
        using var stream = typeof(JsonLocalizer).Assembly.GetManifestResourceStream(name)
            ?? throw new InvalidOperationException($"missing embedded string catalog {name}");
        var all = JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, string>>>(stream)
            ?? throw new InvalidOperationException($"empty string catalog {name}");
        return all.ToDictionary(pair => pair.Key, pair => (IReadOnlyDictionary<string, string>)pair.Value);
    }
}

/// <summary>Named placeholders (<c>{name}</c>), as in the design's strings.json.</summary>
public static class Placeholders
{
    /// <summary>Replaces each <c>{name}</c> with its value; unknown names are left as they are.</summary>
    public static string Fill(string text, params (string Name, object? Value)[] args)
    {
        if (args.Length == 0 || text.IndexOf('{') < 0) return text;
        var builder = new StringBuilder(text.Length + 16);
        var i = 0;
        while (i < text.Length)
        {
            var open = text.IndexOf('{', i);
            if (open < 0) break;
            var close = text.IndexOf('}', open + 1);
            if (close < 0) break;
            builder.Append(text, i, open - i);
            var name = text.AsSpan(open + 1, close - open - 1);
            var found = false;
            foreach (var (argName, value) in args)
            {
                if (!name.SequenceEqual(argName)) continue;
                builder.Append(Convert.ToString(value, CultureInfo.CurrentCulture));
                found = true;
                break;
            }
            if (!found) builder.Append(text, open, close - open + 1);
            i = close + 1;
        }
        builder.Append(text, i, text.Length - i);
        return builder.ToString();
    }

    /// <summary>The placeholder names in <paramref name="text"/>.</summary>
    public static IReadOnlySet<string> Names(string text) =>
        Regex.Matches(text, @"\{(\w+)\}").Select(m => m.Groups[1].Value).ToHashSet();
}
