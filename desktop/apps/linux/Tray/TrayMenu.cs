namespace PPVPN.Linux.Tray;

/// <summary>One entry of the tray menu. Immutable; rebuild the tree to change it.</summary>
public sealed record TrayMenuItem(string Label)
{
    public bool Enabled { get; init; } = true;
    public TrayToggle Toggle { get; init; } = TrayToggle.None;
    public bool Checked { get; init; }
    public bool IsSeparator { get; init; }
    public IReadOnlyList<TrayMenuItem> Children { get; init; } = [];
    /// <summary>Runs on the GTK main thread.</summary>
    public Action? Activated { get; init; }

    public static TrayMenuItem Separator { get; } = new("") { IsSeparator = true };
}

public enum TrayToggle
{
    None,
    Checkmark,
    Radio,
}

/// <param name="IconName">An icon in the tray icon theme (Resources/icons).</param>
public sealed record TrayState(string IconName, string Tooltip, IReadOnlyList<TrayMenuItem> Menu);
