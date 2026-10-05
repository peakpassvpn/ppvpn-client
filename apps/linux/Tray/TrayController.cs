using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using AppTrayItem = PPVPN.App.Core.ViewModels.TrayItem;

namespace PPVPN.Linux.Tray;

/// <summary>
/// Renders App.Core's tray model (<see cref="MainViewModel.Tray"/>) as the dbusmenu of the
/// StatusNotifierItem, plus the Linux-only update line. GNOME Shell's AppIndicator expands the
/// node submenu inline, as the design asks.
/// </summary>
public sealed class TrayController(MainViewModel vm, Update.UpdateNotifier updates, Action showMainWindow)
{
    public static readonly string[] UpdateProperties =
        [nameof(Update.UpdateNotifier.IsAvailable), nameof(Update.UpdateNotifier.AvailableVersion)];

    public TrayState Build()
    {
        var model = vm.Tray;
        var menu = model.Items.Select(Convert).ToList();
        if (updates.IsAvailable)
        {
            // Its own line before "Open PPVPN", like the unread line.
            var open = model.Items.ToList().FindIndex(item => item.Role == TrayItemRole.OpenMain);
            var update = new TrayMenuItem(T("updAvailTray", ("v", updates.AvailableVersion)))
            {
                Activated = () =>
                {
                    updates.ShowBanner();
                    showMainWindow();
                },
            };
            menu.Insert(open < 0 ? menu.Count : open, update);
        }
        return new TrayState(IconName(model.Icon), model.ToolTip, menu);
    }

    private static TrayMenuItem Convert(AppTrayItem item)
    {
        if (item.Kind == TrayItemKind.Separator) return TrayMenuItem.Separator;
        var text = item.Secondary is { Length: > 0 } secondary ? $"{item.Text}  ·  {secondary}" : item.Text;
        if (item.Dot is not null || item.BadgeDot) text = "● " + text;
        return new TrayMenuItem(text)
        {
            Enabled = item.IsEnabled && item.Kind is not (TrayItemKind.Header or TrayItemKind.Disabled),
            Toggle = item.Kind == TrayItemKind.Check ? TrayToggle.Checkmark
                : item.Role == TrayItemRole.Node ? TrayToggle.Radio : TrayToggle.None,
            Checked = item.IsChecked,
            Children = item.Children?.Select(Convert).ToList() ?? [],
            Activated = item.Command is { } command ? () => command.Execute(item.CommandParameter) : null,
        };
    }

    private static string IconName(TrayIconState state) => state switch
    {
        TrayIconState.On => "ppvpn-tray-connected",
        TrayIconState.Busy => "ppvpn-tray-connecting",
        TrayIconState.Error => "ppvpn-tray-error",
        _ => "ppvpn-tray-disconnected",
    };
}
