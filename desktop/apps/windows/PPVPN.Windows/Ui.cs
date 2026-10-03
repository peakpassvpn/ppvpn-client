using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using PPVPN.App.Core.ViewModels;
using Windows.UI;

namespace PPVPN.Windows;

/// <summary>x:Bind helper functions (view-side mapping of view-model values).</summary>
public static class Ui
{
    /// <summary>The main window root; its ActualTheme decides tone colours resolved in code.</summary>
    internal static FrameworkElement? Root { get; set; }

    static bool IsDark => Root?.ActualTheme == ElementTheme.Dark;

    public static Visibility Visible(bool value) => value ? Visibility.Visible : Visibility.Collapsed;

    public static Visibility Collapsed(bool value) => value ? Visibility.Collapsed : Visibility.Visible;

    public static Visibility VisibleText(string? value) => string.IsNullOrEmpty(value) ? Visibility.Collapsed : Visibility.Visible;

    public static bool Not(bool value) => !value;

    /// <summary>The password eye toggle: "show" while hidden, "hide" while revealed.</summary>
    public static string EyeGlyph(bool revealed) => revealed ? "\uED1A" : "\uE890";

    public static string EyeText(bool revealed) => Strings.Loc.Get(revealed ? "hidePassword" : "showPassword");

    public static bool And(bool a, bool b) => a && b;

    /// <summary>Visible when <paramref name="a"/> and not <paramref name="b"/>.</summary>
    public static Visibility VisibleUnless(bool a, bool b) => a && !b ? Visibility.Visible : Visibility.Collapsed;

    public static Visibility VisibleBoth(bool a, bool b) => a && b ? Visibility.Visible : Visibility.Collapsed;

    public static ConnectionTone ToTone(StatusTone tone) => tone switch
    {
        StatusTone.Good => ConnectionTone.Ok,
        StatusTone.Busy => ConnectionTone.Busy,
        StatusTone.Caution => ConnectionTone.Warn,
        StatusTone.Bad => ConnectionTone.Error,
        _ => ConnectionTone.Idle,
    };

    /// <summary>
    /// Theme-aware tone colour (design tokens). Resolved in code because ThemeDictionaries
    /// looked up from code ignore a per-window RequestedTheme; pages call Bindings.Update()
    /// when the theme changes.
    /// </summary>
    public static Color ToneColor(ConnectionTone tone) => (tone, IsDark) switch
    {
        (ConnectionTone.Ok, false) => Rgb(0x16, 0xA3, 0x4A),
        (ConnectionTone.Ok, true) => Rgb(0x4A, 0xDE, 0x80),
        (ConnectionTone.Busy, false) => Rgb(0x1F, 0x43, 0x90),
        (ConnectionTone.Busy, true) => Rgb(0xB4, 0xC5, 0xFF),
        (ConnectionTone.Warn, false) => Rgb(0xB4, 0x53, 0x09),
        (ConnectionTone.Warn, true) => Rgb(0xFF, 0xB5, 0x9D),
        (ConnectionTone.Error, false) => Rgb(0xBA, 0x1A, 0x1A),
        (ConnectionTone.Error, true) => Rgb(0xFF, 0xB4, 0xAB),
        (_, false) => Color.FromArgb(0x9E, 0, 0, 0),
        (_, true) => Color.FromArgb(0xC8, 0xFF, 0xFF, 0xFF),
    };

    public static Brush ToneBrush(ConnectionTone tone) => new SolidColorBrush(ToneColor(tone));

    /// <summary>Tone-soft background (status circle, notices).</summary>
    public static Brush ToneSoftBrush(ConnectionTone tone)
    {
        if (tone == ConnectionTone.Idle)
            return new SolidColorBrush(IsDark ? Color.FromArgb(0x18, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x0C, 0, 0, 0));
        var color = tone == ConnectionTone.Warn && !IsDark ? Rgb(0xD9, 0x77, 0x06) : ToneColor(tone);
        var alpha = (byte)(IsDark ? 0x29 : tone switch { ConnectionTone.Error => 0x1A, ConnectionTone.Warn => 0x21, _ => 0x1F });
        return new SolidColorBrush(Color.FromArgb(alpha, color.R, color.G, color.B));
    }

    /// <summary>Title colour of the connection card: danger on error, otherwise the primary text.</summary>
    public static Brush TitleBrush(ConnectionTone tone) =>
        tone == ConnectionTone.Error ? ToneBrush(ConnectionTone.Error) : new SolidColorBrush(IsDark ? Colors.White : Color.FromArgb(0xE4, 0, 0, 0));

    public static Brush StatusToneBrush(StatusTone tone) => ToneBrush(ToTone(tone));

    public static global::Windows.UI.Text.FontWeight Weight(bool strong) =>
        strong ? Microsoft.UI.Text.FontWeights.SemiBold : Microsoft.UI.Text.FontWeights.Normal;

    public static global::Windows.UI.Text.FontWeight ReadWeight(bool read) =>
        read ? Microsoft.UI.Text.FontWeights.Normal : Microsoft.UI.Text.FontWeights.Bold;

    public static Brush Transparent { get; } = new SolidColorBrush(Colors.Transparent);

    static readonly Dictionary<string, ImageSource> Flags = new(StringComparer.OrdinalIgnoreCase);

    static readonly HashSet<string> KnownFlags = new(StringComparer.OrdinalIgnoreCase)
    {
        "au", "ca", "cn", "de", "fr", "gb", "hk", "jp", "kr", "nl", "sg", "tw", "us",
    };

    /// <summary>4:3 flag for an ISO 3166 alpha-2 code (Assets/Flags/*.svg); a neutral globe for others.</summary>
    public static ImageSource Flag(string? countryCode)
    {
        var code = string.IsNullOrWhiteSpace(countryCode) ? "xx" : countryCode.Trim().ToLowerInvariant();
        if (!KnownFlags.Contains(code)) code = "xx";
        if (!Flags.TryGetValue(code, out var source))
        {
            source = new SvgImageSource(new Uri($"ms-appx:///Assets/Flags/{code}.svg"));
            Flags[code] = source;
        }
        return source;
    }

    static Color Rgb(byte r, byte g, byte b) => Color.FromArgb(255, r, g, b);
}
