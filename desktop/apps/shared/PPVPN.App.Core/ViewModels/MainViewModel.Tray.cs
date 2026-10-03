using System.Windows.Input;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.App.Core.Backend;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>Tray / indicator icon (monochrome, shape only, never a badge).</summary>
public enum TrayIconState
{
    /// <summary>Outline: both modes off, signed out, or restricted.</summary>
    Off,
    /// <summary>Outline + 2-frame breathing.</summary>
    Busy,
    /// <summary>Solid: connected.</summary>
    On,
    /// <summary>Outline + "!" corner: occupied, failed.</summary>
    Error,
}

public enum TrayItemKind
{
    /// <summary>Status header (disabled): <see cref="TrayItem.Text"/> = title, <see cref="TrayItem.Secondary"/> = "node · ms", <see cref="TrayItem.Dot"/>.</summary>
    Header,
    Action,
    /// <summary>A checkable item; <see cref="TrayItem.Secondary"/> is the state text (right-aligned).</summary>
    Check,
    Submenu,
    Separator,
    /// <summary>A disabled line (restricted title, "Not Signed In").</summary>
    Disabled,
}

/// <summary>What an item is, for platform-specific icons, shortcuts or texts.</summary>
public enum TrayItemRole
{
    None, Status, Connect, CurrentNode, Node, Messages, OpenMain, Settings, CheckUpdates, Quit, Restricted, SignedOut,
}

/// <summary>
/// One item of the tray / indicator menu. Plain data so each platform can render it natively
/// (Win32 popup menu, StatusNotifierItem DBusMenu): text, optional right-aligned secondary text,
/// check state, enabled state, command, children.
/// </summary>
public sealed record TrayItem(
    TrayItemKind Kind,
    TrayItemRole Role,
    string Text = "",
    string? Secondary = null,
    bool IsChecked = false,
    bool IsEnabled = true,
    ICommand? Command = null,
    object? CommandParameter = null,
    IReadOnlyList<TrayItem>? Children = null,
    ConnectionTone? Dot = null,
    bool BadgeDot = false)
{
    public static TrayItem Separator { get; } = new(TrayItemKind.Separator, TrayItemRole.None);
}

/// <summary>The whole tray menu plus the icon state and tooltip; rebuilt whenever the state changes.</summary>
public sealed record TrayMenu(TrayIconState Icon, string ToolTip, IReadOnlyList<TrayItem> Items);

public sealed partial class MainViewModel
{
    /// <summary>The tray menu model (core.js <c>trayBase</c>). Windows rebuilds its popup from it on open.</summary>
    [ObservableProperty] TrayMenu tray = new(TrayIconState.Off, "PPVPN", []);

    /// <summary>The Settings item's key: <c>settingsMenuWin</c> (Windows), <c>preferences</c> (Linux).</summary>
    public string SettingsMenuKey { get; set; } = "settingsMenuWin";

    /// <summary>Select a node from the tray's node submenu (parameter: node id).</summary>
    [RelayCommand]
    Task SelectNodeFromTrayAsync(string? nodeId) =>
        nodeId is null || nodeId == Snapshot.SelectedNodeId ? Task.CompletedTask : SelectNodeAsync(nodeId);

    void RefreshTray()
    {
        var signedIn = Snapshot.Auth is AuthState.SignedIn;
        var items = new List<TrayItem>();
        var messages = new TrayItem(TrayItemKind.Action, TrayItemRole.Messages,
            UnreadNotifications > 0 ? _strings.Format("unreadN", ("n", UnreadBadgeText)) : _strings.Get("noUnread"),
            Command: OpenMessagesCommand, BadgeDot: UnreadNotifications > 0);

        if (signedIn && !IsRestricted)
        {
            items.Add(new(TrayItemKind.Header, TrayItemRole.Status, ConnectionTitle,
                CurrentNodeName.Length > 0 ? $"{CurrentNodeName} · {CurrentNodeLatencyText}" : null,
                IsEnabled: false, Dot: ConnectionTone));
            items.Add(TrayItem.Separator);
            items.Add(new(TrayItemKind.Check, TrayItemRole.Connect, _strings.Get("connect"),
                ConnectionMode == ConnectionMode.Enhanced ? _strings.Get($"st_{StateKey(ConnectState)}") : _strings.Get($"std_{StdKey(ConnectState)}"),
                IsChecked: ConnectState is ConnectState.Preparing or ConnectState.Authorizing or ConnectState.Connecting or ConnectState.On or ConnectState.Reconnecting,
                IsEnabled: ConnectSwitchEnabled, Command: ToggleConnectCommand));
            var nodes = Nodes.Items.Select(n => new TrayItem(TrayItemKind.Check, TrayItemRole.Node, n.Name, n.LatencyText,
                IsChecked: n.IsCurrent, Command: SelectNodeFromTrayCommand, CommandParameter: n.Id)).ToList();
            items.Add(new(TrayItemKind.Submenu, TrayItemRole.CurrentNode,
                _strings.Format("currentNodeIs", ("n", CurrentNodeName.Length > 0 ? CurrentNodeName : "—")),
                IsEnabled: nodes.Count > 0, Children: nodes));
            items.Add(TrayItem.Separator);
            items.Add(messages);
        }
        else if (signedIn)
        {
            items.Add(new(TrayItemKind.Disabled, TrayItemRole.Restricted, RestrictedTitle, IsEnabled: false));
            items.Add(TrayItem.Separator);
            items.Add(messages);
        }
        else if (Snapshot.Auth is not AuthState.Restoring)
        {
            items.Add(new(TrayItemKind.Disabled, TrayItemRole.SignedOut, _strings.Get("notSignedIn"), IsEnabled: false));
        }
        items.Add(TrayItem.Separator);
        items.Add(new(TrayItemKind.Action, TrayItemRole.OpenMain, _strings.Get("openMain"), Command: OpenMainCommand));
        items.Add(new(TrayItemKind.Action, TrayItemRole.Settings, _strings.Get(SettingsMenuKey), Command: OpenSettingsCommand));
        if (CanCheckForUpdates)
            items.Add(new(TrayItemKind.Action, TrayItemRole.CheckUpdates, _strings.Get("checkUpdates"), Command: CheckForUpdatesCommand));
        items.Add(TrayItem.Separator);
        items.Add(new(TrayItemKind.Action, TrayItemRole.Quit, _strings.Get("quit"), Command: QuitCommand));

        var icon = !signedIn || IsRestricted ? TrayIconState.Off : ConnectionTone switch
        {
            ConnectionTone.Ok => TrayIconState.On,
            ConnectionTone.Busy => TrayIconState.Busy,
            ConnectionTone.Error or ConnectionTone.Warn => TrayIconState.Error,
            _ => TrayIconState.Off,
        };
        var toolTip = signedIn ? $"PPVPN · {StatusLine}"
            : Snapshot.Auth is AuthState.Restoring ? "PPVPN"
            : $"PPVPN · {_strings.Get("notSignedIn")}";
        var menu = new TrayMenu(icon, toolTip, items);
        if (menu.Icon != Tray.Icon || menu.ToolTip != Tray.ToolTip || !menu.Items.SequenceEqual(Tray.Items, TrayItemComparer.Instance))
            Tray = menu;
    }

    sealed class TrayItemComparer : IEqualityComparer<TrayItem>
    {
        public static TrayItemComparer Instance { get; } = new();
        public bool Equals(TrayItem? x, TrayItem? y) =>
            x is not null && y is not null
            && x with { Children = null } == y with { Children = null }
            && (x.Children ?? []).SequenceEqual(y.Children ?? [], this);
        public int GetHashCode(TrayItem item) => item.Text.GetHashCode();
    }
}
