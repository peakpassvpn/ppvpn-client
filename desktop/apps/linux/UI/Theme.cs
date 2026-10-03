using PPVPN.App.Core.ViewModels;

namespace PPVPN.Linux.UI;

/// <summary>
/// The design's brand and semantic colours on top of libadwaita's own surfaces: the accent
/// through @define-color accent_bg_color (libadwaita 1.1 honours it), the rest as classes.
/// Surfaces, fonts and controls stay the system's. Dark mode swaps the palette.
/// </summary>
public static class Theme
{
    private const string Light = """
        @define-color accent_bg_color #1f4390;
        @define-color accent_fg_color #ffffff;
        @define-color accent_color #1f4390;
        @define-color ppvpn_accent_soft rgba(31,67,144,0.12);
        @define-color ppvpn_success #16a34a;
        @define-color ppvpn_success_soft rgba(22,163,74,0.12);
        @define-color ppvpn_warning #b45309;
        @define-color ppvpn_warning_soft rgba(217,119,6,0.13);
        @define-color ppvpn_danger #ba1a1a;
        @define-color ppvpn_danger_soft rgba(186,26,26,0.10);
        @define-color ppvpn_idle rgba(0,0,0,0.55);
        @define-color ppvpn_badge_bg #ba1a1a;
        @define-color ppvpn_badge_fg #ffffff;
        """;

    private const string Dark = """
        @define-color accent_bg_color #b4c5ff;
        @define-color accent_fg_color #002a78;
        @define-color accent_color #b4c5ff;
        @define-color ppvpn_accent_soft rgba(180,197,255,0.16);
        @define-color ppvpn_success #4ade80;
        @define-color ppvpn_success_soft rgba(74,222,128,0.14);
        @define-color ppvpn_warning #ffb59d;
        @define-color ppvpn_warning_soft rgba(255,181,157,0.14);
        @define-color ppvpn_danger #ffb4ab;
        @define-color ppvpn_danger_soft rgba(255,180,171,0.14);
        @define-color ppvpn_idle rgba(255,255,255,0.55);
        @define-color ppvpn_badge_bg #ffb4ab;
        @define-color ppvpn_badge_fg #690005;
        """;

    private const string Components = """
        headerbar.ppvpn-main { min-height: 46px; }
        .ppvpn-status-dot { font-size: 10px; }
        .ppvpn-status-line { font-weight: 600; }

        .tone-ok { color: @ppvpn_success; }
        .tone-busy { color: @accent_color; }
        .tone-warn { color: @ppvpn_warning; }
        .tone-error { color: @ppvpn_danger; }
        .tone-idle { color: @ppvpn_idle; }

        .ppvpn-weakest { opacity: 0.55; font-size: 0.92em; }
        .ppvpn-mono { font-family: monospace; }

        /* Connection card */
        .ppvpn-card { background-color: @card_bg_color; border-radius: 12px; }
        .ppvpn-card-header { padding: 18px 18px 6px 18px; }
        .ppvpn-status-circle { min-width: 46px; min-height: 46px; border-radius: 23px; }
        .ppvpn-status-circle.tone-ok { background-color: @ppvpn_success_soft; }
        .ppvpn-status-circle.tone-busy { background-color: @ppvpn_accent_soft; }
        .ppvpn-status-circle.tone-warn { background-color: @ppvpn_warning_soft; }
        .ppvpn-status-circle.tone-error { background-color: @ppvpn_danger_soft; }
        .ppvpn-status-circle.tone-idle { background-color: alpha(currentColor, 0.08); }
        .ppvpn-status-title { font-size: 22px; font-weight: 900; }
        .ppvpn-caption { padding: 0 18px 16px 82px; }
        .ppvpn-install-hint { background-color: @ppvpn_accent_soft; border-radius: 8px; padding: 10px 12px; margin: 0 12px 12px 12px; }

        /* Switch, three states: the in-between one centres the knob over a soft track. */
        switch.ppvpn-busy { background-color: @ppvpn_accent_soft; border: 1px solid @accent_color; }
        switch.ppvpn-busy slider { margin-left: 11px; }

        /* Notices under the card */
        .ppvpn-notice { border-radius: 12px; padding: 12px 14px; }
        .ppvpn-notice.tone-error { background-color: @ppvpn_danger_soft; }
        .ppvpn-notice.tone-warn { background-color: @ppvpn_warning_soft; }
        .ppvpn-notice .ppvpn-notice-title { font-weight: 700; }

        /* Traffic */
        .ppvpn-rate { font-family: monospace; font-size: 24px; font-weight: 600; }

        /* Top-of-content warning (AdwBanner is libadwaita 1.3) */
        .ppvpn-banner { padding: 8px 14px; background-color: @ppvpn_warning_soft; }
        .ppvpn-banner.error { background-color: @ppvpn_danger_soft; }

        /* Unread badge on the bell */
        .ppvpn-badge { background-color: @ppvpn_badge_bg; color: @ppvpn_badge_fg; border-radius: 9px;
                       min-width: 18px; min-height: 18px; padding: 0 4px; font-size: 11px; font-weight: 700;
                       font-feature-settings: "tnum"; box-shadow: 0 0 0 2px @headerbar_bg_color; }

        /* Account button */
        .ppvpn-avatar { background-color: @accent_bg_color; color: @accent_fg_color; border-radius: 12px;
                        min-width: 24px; min-height: 24px; font-weight: 700; font-size: 12px; }

        /* Tier tag in the node table */
        .ppvpn-tag { background-color: @ppvpn_accent_soft; color: @accent_color; border-radius: 6px;
                     padding: 1px 6px; font-size: 0.85em; }
        .ppvpn-table-header { font-size: 0.85em; opacity: 0.6; padding: 6px 12px; }
        .ppvpn-current { font-weight: 600; }

        /* Logs */
        .ppvpn-log { font-size: 12px; }
        .ppvpn-live-dot { color: @ppvpn_success; }

        /* Messages */
        .ppvpn-msg-bar { min-width: 3px; }
        .ppvpn-msg-bar.tone-error { background-color: @ppvpn_danger; }
        .ppvpn-msg-bar.tone-warn { background-color: @ppvpn_warning; }
        .ppvpn-unread-dot { background-color: @accent_color; min-width: 8px; min-height: 8px; border-radius: 4px; }
        .ppvpn-type-icon { min-width: 32px; min-height: 32px; border-radius: 16px; background-color: alpha(currentColor, 0.08); }
        .ppvpn-type-icon.tone-error { color: @ppvpn_danger; background-color: @ppvpn_danger_soft; }
        .ppvpn-type-icon.tone-warn { color: @ppvpn_warning; background-color: @ppvpn_warning_soft; }
        .ppvpn-msg-title-unread { font-weight: 700; }
        .ppvpn-skeleton { background-color: alpha(currentColor, 0.08); border-radius: 4px; min-height: 10px; }
        .ppvpn-level-tag { border: 1px solid currentColor; border-radius: 6px; padding: 0 6px; font-size: 0.85em; }

        .ppvpn-alert { border-radius: 15px; }
        """;

    private static Gtk.CssProvider? _palette;

    /// <summary>Loads the component styles and the palette for the current light/dark mode.</summary>
    public static void Install()
    {
        var display = Gdk.Display.GetDefault()!;
        var components = Gtk.CssProvider.New();
        components.LoadFromData(Components, -1);
        Gtk.StyleContext.AddProviderForDisplay(display, components, Gtk.Constants.STYLE_PROVIDER_PRIORITY_APPLICATION);

        var styles = Adw.StyleManager.GetDefault();
        void Apply()
        {
            if (_palette is not null) Gtk.StyleContext.RemoveProviderForDisplay(display, _palette);
            _palette = Gtk.CssProvider.New();
            _palette.LoadFromData(styles.GetDark() ? Dark : Light, -1);
            // Above the theme, below the component rules, which use these colours.
            Gtk.StyleContext.AddProviderForDisplay(display, _palette, Gtk.Constants.STYLE_PROVIDER_PRIORITY_APPLICATION - 1);
        }
        styles.OnNotify += (_, args) =>
        {
            if (args.Pspec.GetName() == "dark") Apply();
        };
        Apply();
    }

    /// <summary>
    /// The palette's warning and danger colours, and dim text (ppvpn_idle over the view
    /// background), as markup values for text (log lines) that CSS classes do not reach; a text
    /// view ignores markup alpha.
    /// </summary>
    public static (string Warning, string Danger, string Dim) MarkupColors =>
        Adw.StyleManager.GetDefault().GetDark()
            ? ("#ffb59d", "#ffb4ab", "#9a9a9a")
            : ("#b45309", "#ba1a1a", "#737373");

    public static string ToneClass(ConnectionTone tone) => tone switch
    {
        ConnectionTone.Ok => "tone-ok",
        ConnectionTone.Busy => "tone-busy",
        ConnectionTone.Warn => "tone-warn",
        ConnectionTone.Error => "tone-error",
        _ => "tone-idle",
    };

    public static string ToneClass(StatusTone tone) => tone switch
    {
        StatusTone.Good => "tone-ok",
        StatusTone.Busy => "tone-busy",
        StatusTone.Caution => "tone-warn",
        StatusTone.Bad => "tone-error",
        _ => "tone-idle",
    };

    private static readonly string[] ToneClasses = ["tone-ok", "tone-busy", "tone-warn", "tone-error", "tone-idle"];

    /// <summary>Replaces the widget's tone class.</summary>
    public static void SetToneClass(this Gtk.Widget widget, string toneClass)
    {
        foreach (var name in ToneClasses) widget.RemoveCssClass(name);
        widget.AddCssClass(toneClass);
    }
}
