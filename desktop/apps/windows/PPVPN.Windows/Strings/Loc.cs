using System.Text.Json;
using Microsoft.UI.Xaml.Markup;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Windows.Strings;

/// <summary>
/// UI strings of the views: App.Core's shared catalog (<see cref="JsonLocalizer"/>, the design's
/// strings.json plus Error_*), read at runtime — there is no resw. A few Windows-only texts
/// (the message window title, "copy address", …) live in <c>Strings/strings.windows.json</c>
/// (same <c>{ "zh": {…}, "en": {…} }</c> shape) and are looked up first.
/// </summary>
public static class Loc
{
    static ILocalizer? _strings;
    static readonly Lazy<Dictionary<string, Dictionary<string, string>>> Extras = new(ReadExtras);

    /// <summary>The app's localizer (set at startup; a default one until then).</summary>
    public static ILocalizer Strings
    {
        get => _strings ??= new JsonLocalizer();
        set => _strings = value;
    }

    /// <summary>"zh" or "en".</summary>
    public static string Language => Strings.Language;

    public static bool IsChinese => Language == JsonLocalizer.Chinese;

    public static string Get(string key)
    {
        var extras = Extras.Value;
        if (extras.TryGetValue(Language, out var local) && local.TryGetValue(key, out var text)) return text;
        if (extras.TryGetValue(JsonLocalizer.English, out var english) && english.TryGetValue(key, out text)) return text;
        return Strings.Get(key);
    }

    /// <summary>Named placeholders: <c>Loc.Format("nodeCount", ("n", 12))</c>.</summary>
    public static string Format(string key, params (string Name, object? Value)[] values) => Placeholders.Fill(Get(key), values);

    static Dictionary<string, Dictionary<string, string>> ReadExtras()
    {
        const string name = "PPVPN.Windows.Strings.strings.windows.json";
        using var stream = typeof(Loc).Assembly.GetManifestResourceStream(name)
            ?? throw new InvalidOperationException($"missing embedded string catalog {name}");
        return JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, string>>>(stream) ?? [];
    }
}

/// <summary><c>Text="{loc:S Key=overview}"</c>: a UI string from <see cref="Loc"/>.</summary>
[MarkupExtensionReturnType(ReturnType = typeof(string))]
public sealed partial class S : MarkupExtension
{
    public string Key { get; set; } = "";

    /// <summary>Optional suffix, e.g. "…".</summary>
    public string Suffix { get; set; } = "";

    protected override object ProvideValue() => Loc.Get(Key) + Suffix;
}
