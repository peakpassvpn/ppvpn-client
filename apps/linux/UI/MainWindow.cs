using PPVPN.App.Core.ViewModels;
using PPVPN.Linux.Update;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// The main window: header bar (status line · Overview / Nodes / Logs · bell, account,
/// preferences), the configuration and update banners, then sign-in or the three pages.
/// libadwaita 1.1 has no ToolbarView, so the header bar is stacked above the content by hand.
/// </summary>
public sealed class MainWindow
{
    public const string Overview = "overview";
    public const string Nodes = "nodes";
    public const string Logs = "logs";

    private readonly MainViewModel _vm;
    private readonly Adw.ViewStack _pages = Adw.ViewStack.New();
    private readonly Gtk.MenuButton _account;
    private readonly Adw.ToastOverlay _toasts = Adw.ToastOverlay.New();

    public MainWindow(Adw.Application app, MainViewModel vm, UpdateNotifier updates, IAppServices services)
    {
        _vm = vm;
        Window = Adw.ApplicationWindow.New(app);
        Window.SetTitle("PPVPN");
        // Wide enough for the page switcher's labels in every language: the header centres it
        // between the status line and the buttons on the right, and at 800 px the English labels
        // ("Overview", "Nodes", "Logs") were cut to "O…".
        Window.SetDefaultSize(900, 580);
        Window.SetSizeRequest(600, 460);

        _account = AccountMenu.Create(vm);
        _pages.AddTitled(OverviewPage.Create(vm, OpenAccountMenu), Overview, T("overview"))?.SetIconName("go-home-symbolic");
        _pages.AddTitled(NodesPage.Create(vm.Nodes), Nodes, T("nodes"))?.SetIconName("network-workgroup-symbolic");
        _pages.AddTitled(LogsPage.Create(vm.Logs), Logs, T("logs"))?.SetIconName("text-x-generic-symbolic");

        var root = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        root.Append(Header());
        root.Append(ConfigBanner());
        root.Append(UpdateBanner(updates, services));
        root.Append(Content());
        _toasts.SetChild(root);
        Window.SetContent(_toasts);
        Window.AddController(Shortcuts());
    }

    public Adw.ApplicationWindow Window { get; }

    public void ShowPage(string name) => _pages.SetVisibleChildName(name);

    public void OpenAccountMenu() => _account.Popup();

    public void Toast(string text) => _toasts.AddToast(Adw.Toast.New(text));

    private Gtk.Widget Header()
    {
        var header = Adw.HeaderBar.New();
        header.AddCssClass("ppvpn-main");

        // Start: the status line, on every page.
        var status = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        status.SetMarginStart(4);
        var dot = Gtk.Label.New("●");
        dot.AddCssClass("ppvpn-status-dot");
        var line = Gtk.Label.New(null);
        line.AddCssClass("ppvpn-status-line");
        // An ellipsizing label asks for almost no width: reserve room for "已连接 · 香港 01".
        line.SetEllipsize(Pango.EllipsizeMode.End);
        line.SetWidthChars(20);
        line.SetMaxWidthChars(22);
        status.Append(dot);
        status.Append(line);
        header.PackStart(status);

        var switcher = Adw.ViewSwitcher.New();
        switcher.SetStack(_pages);
        switcher.SetPolicy(Adw.ViewSwitcherPolicy.Wide);
        var title = Adw.WindowTitle.New("PPVPN", "");

        // End, right to left: preferences, account, bell (with the unread badge).
        header.PackEnd(IconButton("emblem-system-symbolic", T("preferences"), () => _vm.OpenSettingsCommand.Execute()));
        header.PackEnd(_account);
        var bell = Gtk.Overlay.New();
        var bellButton = IconButton("preferences-system-notifications-symbolic", T("msgCenter"), () => _vm.OpenMessagesCommand.Execute());
        bell.SetChild(bellButton);
        var badge = Gtk.Label.New(null);
        badge.AddCssClass("ppvpn-badge");
        badge.SetHalign(Gtk.Align.End);
        badge.SetValign(Gtk.Align.Start);
        badge.SetCanTarget(false);
        bell.AddOverlay(badge);
        header.PackEnd(bell);

        _vm.Bind(() =>
        {
            dot.SetToneClass(Theme.ToneClass(_vm.StatusDotTone));
            line.SetText(_vm.StatusLine);
            status.SetVisible(!_vm.ShowLogin && !_vm.IsRestoring);
            header.SetTitleWidget(_vm.IsSignedIn ? switcher : title);
            bell.SetVisible(_vm.IsSignedIn);
            _account.SetVisible(_vm.IsSignedIn);
            badge.SetVisible(_vm.HasUnreadNotifications);
            badge.SetText(_vm.UnreadBadgeText);
        }, nameof(_vm.StatusDotTone), nameof(_vm.StatusLine), nameof(_vm.Stage), nameof(_vm.HasUnreadNotifications),
            nameof(_vm.UnreadBadgeText));
        return header;
    }

    /// <summary>The configuration is invalid; the previous one stays in use (AdwBanner is 1.3).</summary>
    private Gtk.Widget ConfigBanner()
    {
        var revealer = Gtk.Revealer.New();
        var bar = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        bar.AddCssClass("ppvpn-banner");
        var text = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        text.SetHexpand(true);
        text.Append(Label(T("cfgT"), "heading"));
        var message = Label(T("cfgD"));
        message.SetWrap(true);
        text.Append(message);
        bar.Append(Gtk.Image.NewFromIconName("dialog-warning-symbolic"));
        bar.Append(text);
        bar.Append(TextButton(T("refreshNodes"), () => _vm.RefreshProfileCommand.Execute()));
        revealer.SetChild(bar);
        _vm.Bind(() => revealer.SetRevealChild(_vm.ConfigInvalid && _vm.IsSignedIn), nameof(_vm.ConfigInvalid), nameof(_vm.Stage));
        return revealer;
    }

    /// <summary>A newer release is in the repository: the upgrade command to run.</summary>
    private static Gtk.Widget UpdateBanner(UpdateNotifier updates, IAppServices services)
    {
        var revealer = Gtk.Revealer.New();
        var bar = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        bar.AddCssClass("ppvpn-banner");
        var text = Gtk.Box.New(Gtk.Orientation.Vertical, 2);
        text.SetHexpand(true);
        var title = Label(null, "heading");
        var hint = Label(T("updAvailD"));
        var command = Label(updates.Command, "ppvpn-mono");
        command.SetSelectable(true);
        command.SetWrap(true);
        text.Append(title);
        text.Append(hint);
        text.Append(command);
        bar.Append(text);
        bar.Append(CopyButton(() => new CopyText(services, updates.Command), T("copyCommand")));
        bar.Append(TextButton(T("later"), updates.Later));
        revealer.SetChild(bar);
        updates.Bind(() =>
        {
            revealer.SetRevealChild(updates.IsBannerVisible);
            title.SetText(T("updAvailT", ("v", updates.AvailableVersion)));
        }, nameof(updates.IsBannerVisible), nameof(updates.AvailableVersion));
        return revealer;
    }

    private sealed class CopyText(IAppServices services, string text) : System.Windows.Input.ICommand
    {
        public event EventHandler? CanExecuteChanged { add { } remove { } }
        public bool CanExecute(object? parameter) => true;
        public void Execute(object? parameter) => services.CopyText(text);
    }

    private Gtk.Widget Content()
    {
        var restoring = Gtk.Spinner.New();
        restoring.SetSizeRequest(32, 32);
        restoring.SetHalign(Gtk.Align.Center);
        restoring.SetValign(Gtk.Align.Center);
        var content = Gtk.Stack.New();
        content.SetVexpand(true);
        content.SetTransitionType(Gtk.StackTransitionType.Crossfade);
        content.AddNamed(restoring, "restoring");
        content.AddNamed(LoginPage.Create(_vm), "login");
        content.AddNamed(_pages, "main");
        _vm.Bind(() =>
        {
            content.SetVisibleChildName(_vm.IsRestoring ? "restoring" : _vm.IsSignedIn ? "main" : "login");
            restoring.SetSpinning(_vm.IsRestoring);
        }, nameof(_vm.Stage));
        return content;
    }

    /// <summary>Alt+1/2/3 switch pages; F5 refreshes the page (profile, or the log).</summary>
    private Gtk.EventControllerKey Shortcuts()
    {
        var keys = Gtk.EventControllerKey.New();
        keys.OnKeyPressed += (_, args) =>
        {
            var alt = (args.State & Gdk.ModifierType.AltMask) != 0;
            if (alt && _vm.IsSignedIn)
            {
                var page = args.Keyval switch
                {
                    Gdk.Constants.KEY_1 => Overview,
                    Gdk.Constants.KEY_2 => Nodes,
                    Gdk.Constants.KEY_3 => Logs,
                    _ => null,
                };
                if (page is not null)
                {
                    ShowPage(page);
                    return true;
                }
            }
            if (args.Keyval == Gdk.Constants.KEY_F5 && _vm.IsSignedIn)
            {
                if (_pages.GetVisibleChildName() == Logs) _vm.Logs.ReloadCommand.Execute();
                else _vm.RefreshProfileCommand.Execute();
                return true;
            }
            return false;
        };
        return keys;
    }
}
