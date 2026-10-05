using System.Windows.Input;
using static PPVPN.Linux.UI.L;

namespace PPVPN.Linux.UI;

/// <summary>Small builders for the recurring shapes.</summary>
public static class Widgets
{
    public static Adw.ActionRow Row(string title, string? subtitle = null)
    {
        var row = Adw.ActionRow.New();
        row.SetTitle(title);
        if (subtitle is not null) row.SetSubtitle(subtitle);
        return row;
    }

    /// <summary>A row whose value sits on the right, like a form's labelled content.</summary>
    public static (Adw.ActionRow Row, Gtk.Label Value) ValueRow(string title, bool monospace = false)
    {
        var row = Row(title);
        var value = Gtk.Label.New(null);
        value.SetSelectable(true);
        value.SetEllipsize(Pango.EllipsizeMode.Middle);
        value.SetMaxWidthChars(36);
        value.SetValign(Gtk.Align.Center);
        if (monospace) value.AddCssClass("ppvpn-mono");
        row.AddSuffix(value);
        return (row, value);
    }

    /// <summary>
    /// The local proxy's user name and password rows: both copyable; the password masked until
    /// the eye toggle reveals it (ProxyInfo.PasswordRevealed). Call the returned action whenever
    /// <paramref name="current"/> may have changed; it follows the new ProxyInfo.
    /// </summary>
    public static (Adw.ActionRow User, Adw.ActionRow Password, Action Refresh) CredentialRows(
        Func<PPVPN.App.Core.ViewModels.ProxyInfo?> current)
    {
        var (user, userValue) = ValueRow(T("authUser"), monospace: true);
        user.AddSuffix(CopyButton(() => current()?.CopyUsernameCommand));
        var (password, passwordValue) = ValueRow(T("authPass"), monospace: true);
        var eye = IconButton("view-reveal-symbolic", T("showPassword"), () => current()?.TogglePasswordRevealedCommand.Execute(null));
        password.AddSuffix(eye);
        password.AddSuffix(CopyButton(() => current()?.CopyPasswordCommand));

        PPVPN.App.Core.ViewModels.ProxyInfo? bound = null;
        void Show()
        {
            var proxy = current();
            userValue.SetText(proxy?.Username ?? "");
            passwordValue.SetText(proxy?.PasswordDisplay ?? "");
            var revealed = proxy?.PasswordRevealed == true;
            eye.SetIconName(revealed ? "view-conceal-symbolic" : "view-reveal-symbolic");
            eye.SetTooltipText(T(revealed ? "hidePassword" : "showPassword"));
        }
        void OnChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs args) => Show();
        void Refresh()
        {
            var proxy = current();
            if (!ReferenceEquals(proxy, bound))
            {
                if (bound is not null) bound.PropertyChanged -= OnChanged;
                bound = proxy;
                if (bound is not null) bound.PropertyChanged += OnChanged;
            }
            Show();
        }
        Refresh();
        return (user, password, Refresh);
    }

    public static Gtk.Button IconButton(string icon, string tooltip, Action clicked)
    {
        var button = Gtk.Button.NewFromIconName(icon);
        button.SetTooltipText(tooltip);
        button.SetValign(Gtk.Align.Center);
        button.AddCssClass("flat");
        button.OnClicked += (_, _) => clicked();
        return button;
    }

    public static Gtk.Button TextButton(string label, Action clicked, params string[] css)
    {
        var button = Gtk.Button.NewWithLabel(label);
        button.SetValign(Gtk.Align.Center);
        foreach (var name in css) button.AddCssClass(name);
        button.OnClicked += (_, _) => clicked();
        return button;
    }

    public static void Execute(this ICommand command, object? parameter = null)
    {
        if (command.CanExecute(parameter)) command.Execute(parameter);
    }

    /// <summary>
    /// A copy button that turns into "✓ Copied" for 1.6 s in place (the design's one-time
    /// success feedback; no toast).
    /// </summary>
    public static Gtk.Button CopyButton(Func<ICommand?> command, string? label = null)
    {
        var button = Gtk.Button.New();
        button.AddCssClass("flat");
        button.SetValign(Gtk.Align.Center);
        var content = Adw.ButtonContent.New();
        content.SetIconName("edit-copy-symbolic");
        content.SetLabel(label ?? T("copy"));
        button.SetChild(content);
        var generation = 0;
        button.OnClicked += async (_, _) =>
        {
            if (command() is not { } run) return;
            run.Execute(null);
            var mine = ++generation;
            content.SetIconName("object-select-symbolic");
            content.SetLabel(T("copied"));
            await Task.Delay(1600);
            if (mine != generation) return;
            content.SetIconName("edit-copy-symbolic");
            content.SetLabel(label ?? T("copy"));
        };
        return button;
    }

    public static Adw.PreferencesGroup Group(string? title = null, string? description = null)
    {
        var group = Adw.PreferencesGroup.New();
        if (title is not null) group.SetTitle(title);
        if (description is not null) group.SetDescription(description);
        return group;
    }

    public static Gtk.Label Label(string? text = null, params string[] css)
    {
        var label = Gtk.Label.New(text);
        label.SetXalign(0);
        foreach (var name in css) label.AddCssClass(name);
        return label;
    }

    /// <summary>
    /// A 4:3 flag (flag-icons, MIT; the macOS app's PNGs, copied to Resources/flags), drawn at a
    /// fixed size with rounded corners and a hairline so white flags keep their edge. Unknown
    /// countries leave an empty slot of the same size, so names stay aligned.
    /// </summary>
    public static Gtk.DrawingArea Flag(string? countryCode, int height = 12)
    {
        var area = Gtk.DrawingArea.New();
        area.SetContentWidth(height * 4 / 3);
        area.SetContentHeight(height);
        area.SetValign(Gtk.Align.Center);
        area.SetHalign(Gtk.Align.Start);
        SetFlag(area, countryCode);
        return area;
    }

    /// <summary>Shows <paramref name="countryCode"/>'s flag; false (an empty slot) when there is none.</summary>
    public static bool SetFlag(Gtk.DrawingArea area, string? countryCode)
    {
        var image = FlagImage(countryCode);
        area.SetDrawFunc((_, cr, width, height) =>
        {
            if (image is null) return;
            RoundedRectangle(cr, 0, 0, width, height, 2);
            cr.Save();
            cr.Clip();
            cr.Scale((double)width / image.GetWidth(), (double)height / image.GetHeight());
            Gdk.Functions.CairoSetSourcePixbuf(cr, image, 0, 0);
            cr.Paint();
            cr.Restore();
            RoundedRectangle(cr, 0.5, 0.5, width - 1, height - 1, 1.5);
            var dark = Adw.StyleManager.GetDefault().GetDark();
            cr.SetSourceRgba(dark ? 1 : 0, dark ? 1 : 0, dark ? 1 : 0, dark ? 0.2 : 0.15);
            cr.LineWidth = 1;
            cr.Stroke();
        });
        area.QueueDraw();
        return image is not null;
    }

    private static readonly Dictionary<string, GdkPixbuf.Pixbuf?> FlagImages = [];

    private static GdkPixbuf.Pixbuf? FlagImage(string? countryCode)
    {
        if (FlagPath(countryCode) is not { } path) return null;
        if (!FlagImages.TryGetValue(path, out var image))
        {
            try { image = GdkPixbuf.Pixbuf.NewFromFile(path); }
            catch (Exception) { image = null; }
            FlagImages[path] = image;
        }
        return image;
    }

    private static void RoundedRectangle(Cairo.Context cr, double x, double y, double width, double height, double radius)
    {
        cr.NewSubPath();
        cr.Arc(x + width - radius, y + radius, radius, -Math.PI / 2, 0);
        cr.Arc(x + width - radius, y + height - radius, radius, 0, Math.PI / 2);
        cr.Arc(x + radius, y + height - radius, radius, Math.PI / 2, Math.PI);
        cr.Arc(x + radius, y + radius, radius, Math.PI, 3 * Math.PI / 2);
        cr.ClosePath();
    }

    private static string? FlagPath(string? countryCode)
    {
        if (countryCode is not { Length: 2 } code || !code.All(char.IsAsciiLetter)) return null;
        var path = Path.Combine(AppContext.BaseDirectory, "Resources", "flags", code.ToLowerInvariant() + ".png");
        return File.Exists(path) ? path : null;
    }

    /// <summary>Wraps page content in the design's AdwClamp (620) with 24 × 12 margins.</summary>
    public static Gtk.Widget Clamped(Gtk.Widget content, bool scroll = true)
    {
        var clamp = Adw.Clamp.New();
        clamp.SetMaximumSize(620);
        clamp.SetTighteningThreshold(560);
        content.SetMarginTop(24);
        content.SetMarginBottom(24);
        content.SetMarginStart(12);
        content.SetMarginEnd(12);
        clamp.SetChild(content);
        if (!scroll) return clamp;
        var scroller = Gtk.ScrolledWindow.New();
        scroller.SetPolicy(Gtk.PolicyType.Never, Gtk.PolicyType.Automatic);
        scroller.SetVexpand(true);
        scroller.SetChild(clamp);
        return scroller;
    }

    /// <summary>A switch whose user changes are reported, but not the ones <see cref="Set"/> makes.</summary>
    public sealed class BoundSwitch
    {
        private bool _updating;

        public BoundSwitch(Action<bool> changed)
        {
            Switch = Gtk.Switch.New();
            Switch.SetValign(Gtk.Align.Center);
            Switch.OnStateSet += (_, args) =>
            {
                if (_updating) return false;
                changed(args.State);
                // The view model owns the state: it snaps back through Set() when refused.
                return true;
            };
        }

        public Gtk.Switch Switch { get; }

        public void Set(bool active)
        {
            _updating = true;
            Switch.SetActive(active);
            Switch.SetState(active);
            _updating = false;
        }
    }

    /// <summary>An Adw.ComboRow over a list of titles, reporting user selections only.</summary>
    public sealed class BoundCombo
    {
        private bool _updating;
        private IReadOnlyList<string?> _flags = [];

        /// <param name="flags">Items start with a flag (<see cref="SetItems"/>'s country codes).</param>
        public BoundCombo(string title, Action<int> selected, bool flags = false)
        {
            Row = Adw.ComboRow.New();
            Row.SetTitle(title);
            if (flags) Row.SetFactory(FlagItemFactory());
            Row.OnNotify += (_, args) =>
            {
                if (_updating || args.Pspec.GetName() != "selected") return;
                var index = Row.GetSelected();
                if (index != Gtk.Constants.INVALID_LIST_POSITION) selected((int)index);
            };
        }

        public Adw.ComboRow Row { get; }

        public void SetItems(IEnumerable<string> titles, int selected, IReadOnlyList<string?>? countryCodes = null)
        {
            _updating = true;
            _flags = countryCodes ?? [];
            Row.SetModel(Gtk.StringList.New(titles.ToArray()));
            Row.SetSelected(selected < 0 ? Gtk.Constants.INVALID_LIST_POSITION : (uint)selected);
            _updating = false;
        }

        public void Select(int selected)
        {
            _updating = true;
            Row.SetSelected(selected < 0 ? Gtk.Constants.INVALID_LIST_POSITION : (uint)selected);
            _updating = false;
        }

        /// <summary>Flag + title, for both the popover list and the selected value.</summary>
        private Gtk.SignalListItemFactory FlagItemFactory()
        {
            var factory = Gtk.SignalListItemFactory.New();
            factory.OnSetup += (_, args) =>
            {
                var box = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
                box.Append(Flag(null));
                var label = Gtk.Label.New(null);
                label.SetXalign(0);
                label.SetEllipsize(Pango.EllipsizeMode.End);
                box.Append(label);
                ((Gtk.ListItem)args.Object).SetChild(box);
            };
            factory.OnBind += (_, args) =>
            {
                var item = (Gtk.ListItem)args.Object;
                if (item.GetChild() is not Gtk.Box box) return;
                var position = (int)item.GetPosition();
                SetFlag((Gtk.DrawingArea)box.GetFirstChild()!, position < _flags.Count ? _flags[position] : null);
                ((Gtk.Label)box.GetLastChild()!).SetText((item.GetItem() as Gtk.StringObject)?.GetString() ?? "");
            };
            return factory;
        }
    }
}
