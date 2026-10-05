using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// The header bar's account button (avatar initial + team ▾) and its popover: account, teams
/// (disabled ones greyed with their tag), account settings, sign out.
/// </summary>
public static class AccountMenu
{
    public static Gtk.MenuButton Create(MainViewModel vm)
    {
        var button = Gtk.MenuButton.New();
        button.AddCssClass("flat");
        var face = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        var avatar = Gtk.Label.New(null);
        avatar.AddCssClass("ppvpn-avatar");
        var team = Gtk.Label.New(null);
        team.SetEllipsize(Pango.EllipsizeMode.End);
        team.SetMaxWidthChars(16);
        face.Append(avatar);
        face.Append(team);
        face.Append(Gtk.Image.NewFromIconName("pan-down-symbolic"));
        button.SetChild(face);

        var popover = Gtk.Popover.New();
        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 2);
        box.SetMarginTop(6);
        box.SetMarginBottom(6);
        box.SetSizeRequest(260, -1);
        var head = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        head.SetMarginStart(8);
        head.SetMarginEnd(8);
        head.SetMarginBottom(6);
        var headAvatar = Gtk.Label.New(null);
        headAvatar.AddCssClass("ppvpn-avatar");
        headAvatar.SetValign(Gtk.Align.Center);
        var headText = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        var email = Label(null, "heading");
        email.SetEllipsize(Pango.EllipsizeMode.Middle);
        var subtitle = Label(null, "dim-label", "caption");
        headText.Append(email);
        headText.Append(subtitle);
        head.Append(headAvatar);
        head.Append(headText);
        box.Append(head);
        box.Append(Gtk.Separator.New(Gtk.Orientation.Horizontal));
        var teamsHeader = Label(T("teamsHdr"), "dim-label", "caption");
        teamsHeader.SetMarginStart(10);
        teamsHeader.SetMarginTop(6);
        box.Append(teamsHeader);
        var teams = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        box.Append(teams);
        box.Append(Gtk.Separator.New(Gtk.Orientation.Horizontal));
        box.Append(MenuItem(T("accountSettings"), () =>
        {
            popover.Popdown();
            vm.OpenAccountSettingsCommand.Execute();
        }));
        box.Append(MenuItem(T("signOut") + "…", () =>
        {
            popover.Popdown();
            vm.SignOutCommand.Execute();
        }));
        popover.SetChild(box);
        button.SetPopover(popover);

        vm.Teams.BindItems(() =>
        {
            while (teams.GetFirstChild() is { } child) teams.Remove(child);
            foreach (var option in vm.Teams)
            {
                var item = Gtk.Button.New();
                item.AddCssClass("flat");
                var row = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
                var check = Gtk.Image.NewFromIconName("object-select-symbolic");
                check.SetOpacity(option.IsCurrent ? 1 : 0);
                var name = Label(option.Title);
                name.SetHexpand(true);
                row.Append(check);
                row.Append(name);
                if (option.HasDisabledTag) row.Append(Label(option.DisabledTag, "dim-label", "caption"));
                item.SetChild(row);
                item.SetSensitive(option.IsSelectable);
                var chosen = option;
                item.OnClicked += (_, _) =>
                {
                    popover.Popdown();
                    vm.SwitchTeamCommand.Execute(chosen);
                };
                teams.Append(item);
            }
        });
        vm.Bind(() =>
        {
            avatar.SetText(vm.AvatarInitial);
            headAvatar.SetText(vm.AvatarInitial);
            team.SetText(vm.TeamName);
            // Ellipsizing labels request almost no width; ask for the name's length up to the cap.
            team.SetWidthChars(Math.Min(vm.TeamName.Length, 12));
            email.SetText(vm.AccountEmail);
            subtitle.SetText(vm.AccountSubtitle);
            subtitle.SetVisible(vm.AccountSubtitle.Length > 0);
            button.SetTooltipText(vm.AccountEmail);
        }, nameof(vm.AvatarInitial), nameof(vm.TeamName), nameof(vm.AccountEmail), nameof(vm.AccountSubtitle));
        return button;
    }

    private static Gtk.Button MenuItem(string text, Action clicked)
    {
        var button = Gtk.Button.NewWithLabel(text);
        button.AddCssClass("flat");
        ((Gtk.Label)button.GetChild()!).SetXalign(0);
        button.OnClicked += (_, _) => clicked();
        return button;
    }
}
