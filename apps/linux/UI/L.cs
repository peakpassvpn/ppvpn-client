using PPVPN.App.Core.ViewModels;

namespace PPVPN.Linux.UI;

/// <summary>
/// The shared catalog (App.Core Strings/strings.json through <see cref="JsonLocalizer"/>) for
/// view code: every label, Linux-only ones included, comes from there.
/// </summary>
public static class L
{
    private static ILocalizer? _strings;

    public static ILocalizer Strings => _strings ?? throw new InvalidOperationException("L.Initialize first");

    public static void Initialize(ILocalizer strings) => _strings = strings;

    public static string T(string key) => Strings.Get(key);

    public static string T(string key, params (string Name, object? Value)[] args) => Strings.Format(key, args);
}
