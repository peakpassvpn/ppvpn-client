using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// Preferences: General, Account (signed in only) and Advanced (connection method, API
/// override, system service). AdwPreferencesDialog is libadwaita 1.5, so a PreferencesWindow.
/// </summary>
public static class SettingsWindow
{
    public const string AccountPage = "account";

    public static Adw.PreferencesWindow Create(SettingsViewModel vm, Gtk.Window parent)
    {
        var main = vm.Main;
        var window = Adw.PreferencesWindow.New();
        window.SetTransientFor(parent);
        window.SetTitle(T("preferences"));
        window.SetSearchEnabled(false);
        window.SetDefaultSize(560, 620);
        window.SetHideOnClose(true);
        window.OnShow += (_, _) => vm.Refresh();

        window.Add(General(vm));
        var account = Account(vm);
        window.Add(account);
        window.Add(Advanced(vm));
        vm.Bind(() => account.SetVisible(vm.ShowAccountSection), nameof(vm.ShowAccountSection));
        main.Bind(() => account.SetVisible(vm.ShowAccountSection), nameof(main.Stage));
        return window;
    }

    private static Adw.PreferencesPage General(SettingsViewModel vm)
    {
        var page = Adw.PreferencesPage.New();
        page.SetTitle(T("general"));
        page.SetIconName("emblem-system-symbolic");

        var look = Group();
        var appearance = new BoundCombo(T("appearance"), index => vm.AppearanceIndex = index);
        appearance.SetItems([T("sys"), T("light"), T("dark")], vm.AppearanceIndex);
        vm.Bind(() => appearance.Select(vm.AppearanceIndex), nameof(vm.AppearanceIndex));
        look.Add(appearance.Row);

        var launchRow = Row(T("launchAtLogin"));
        var launch = new BoundSwitch(on => vm.LaunchAtLogin = on);
        launchRow.AddSuffix(launch.Switch);
        launchRow.SetActivatableWidget(launch.Switch);
        vm.Bind(() => launch.Set(vm.LaunchAtLogin), nameof(vm.LaunchAtLogin));
        look.Add(launchRow);
        page.Add(look);

        var updates = Group();
        var autoRow = Row(T("autoUpdate"));
        var auto = new BoundSwitch(on => vm.AutoCheckUpdates = on);
        autoRow.AddSuffix(auto.Switch);
        autoRow.SetActivatableWidget(auto.Switch);
        vm.Bind(() => auto.Set(vm.AutoCheckUpdates), nameof(vm.AutoCheckUpdates));
        updates.Add(autoRow);
        var (versionRow, versionValue) = ValueRow(T("about"));
        versionValue.SetText(vm.VersionText);
        var check = TextButton(T("checkNow"), () => vm.CheckForUpdatesCommand.Execute());
        versionRow.AddSuffix(check);
        updates.Add(versionRow);
        autoRow.SetVisible(vm.ShowUpdates);
        check.SetVisible(vm.ShowUpdates);
        page.Add(updates);
        page.Add(License(vm));
        return page;
    }

    /// <summary>
    /// Under About: the GPL notice (aboutLicense, the same wording on every platform; GPL-3.0
    /// section 5 asks for it, and for the absence of a warranty, in an interactive program), the
    /// copyright line and links to the source code and the license text.
    /// </summary>
    private static Adw.PreferencesGroup License(SettingsViewModel vm)
    {
        var notice = Label(T("aboutLicense"), "dim-label");
        notice.SetWrap(true);
        var links = Gtk.Box.New(Gtk.Orientation.Horizontal, 4);
        links.Append(TextButton(T("aboutSource"), () => vm.OpenSourceCommand.Execute(), "flat"));
        links.Append(TextButton(T("aboutViewLicense"), () => vm.OpenLicenseCommand.Execute(), "flat"));
        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 4);
        box.Append(notice);
        box.Append(Label(SettingsViewModel.Copyright, "dim-label"));
        box.Append(links);
        var group = Group();
        group.Add(box);
        return group;
    }

    private static Adw.PreferencesPage Account(SettingsViewModel vm)
    {
        var main = vm.Main;
        var page = Adw.PreferencesPage.New();
        page.SetName(AccountPage);
        page.SetTitle(T("account"));
        page.SetIconName("avatar-default-symbolic");
        var group = Group();
        var (user, userValue) = ValueRow(T("user"));
        var team = new BoundCombo(T("team"), index =>
        {
            if (index < main.Teams.Count) main.SwitchTeamCommand.Execute(main.Teams[index]);
        });
        var (expires, expiresValue) = ValueRow(T("expires"));
        group.Add(user);
        group.Add(team.Row);
        group.Add(expires);
        page.Add(group);
        var signOutGroup = Group();
        var signOut = TextButton(T("signOut") + "…", () => main.SignOutCommand.Execute(), "destructive-action");
        signOut.SetHalign(Gtk.Align.Start);
        signOutGroup.Add(signOut);
        page.Add(signOutGroup);

        main.Teams.BindItems(() =>
            team.SetItems(main.Teams.Select(t => t.HasDisabledTag ? $"{t.Title} · {t.DisabledTag}" : t.Title),
                main.Teams.ToList().FindIndex(t => t.IsCurrent)));
        main.Bind(() =>
        {
            userValue.SetText(main.AccountEmail);
            expiresValue.SetText(main.ExpiresText);
            if (main.IsExpired) expiresValue.SetToneClass("tone-error");
            else expiresValue.RemoveCssClass("tone-error");
            team.Row.SetSensitive(main.HasTeamChoice);
        }, nameof(main.AccountEmail), nameof(main.ExpiresText), nameof(main.IsExpired), nameof(main.HasTeamChoice));
        return page;
    }

    private static Adw.PreferencesPage Advanced(SettingsViewModel vm)
    {
        var main = vm.Main;
        var page = Adw.PreferencesPage.New();
        page.SetTitle(T("advanced"));
        page.SetIconName("applications-engineering-symbolic");

        // Connection method: Enhanced (TUN, default) or Compatible (system proxy), exclusive.
        var method = Group(T("connMethod"), T("connMethodHint"));
        var radios = new List<(Gtk.CheckButton Radio, ConnectionModeOption Option)>();
        var updating = false;
        foreach (var option in vm.ConnectionModes)
        {
            var row = Row(option.Title, option.Description);
            var radio = Gtk.CheckButton.New();
            if (radios.Count > 0) radio.SetGroup(radios[0].Radio);
            radio.SetValign(Gtk.Align.Center);
            var chosen = option;
            radio.OnToggled += (_, _) =>
            {
                if (!updating && radio.GetActive()) vm.SetConnectionModeCommand.Execute(chosen);
            };
            row.AddPrefix(radio);
            row.SetActivatableWidget(radio);
            radios.Add((radio, option));
            method.Add(row);
        }
        vm.Bind(() =>
        {
            updating = true;
            foreach (var (radio, option) in radios) radio.SetActive(option.Mode == vm.SelectedConnectionMode?.Mode);
            updating = false;
        }, nameof(vm.SelectedConnectionMode));
        main.Bind(() =>
        {
            updating = true;
            foreach (var (radio, option) in radios) radio.SetActive(option.Mode == main.ConnectionMode);
            updating = false;
        }, nameof(main.ConnectionMode));
        page.Add(method);

        // Routing: Rules (default) or Global, exclusive; applied without a reconnect.
        var routing = Group(T("routingMode"));
        var routingRadios = new List<(Gtk.CheckButton Radio, RoutingModeOption Option)>();
        foreach (var option in vm.RoutingModes)
        {
            var row = Row(option.Title, option.Description);
            var radio = Gtk.CheckButton.New();
            if (routingRadios.Count > 0) radio.SetGroup(routingRadios[0].Radio);
            radio.SetValign(Gtk.Align.Center);
            var chosen = option;
            radio.OnToggled += (_, _) =>
            {
                if (!updating && radio.GetActive()) vm.SetRoutingModeCommand.Execute(chosen);
            };
            row.AddPrefix(radio);
            row.SetActivatableWidget(radio);
            routingRadios.Add((radio, option));
            routing.Add(row);
        }
        main.Bind(() =>
        {
            updating = true;
            foreach (var (radio, option) in routingRadios) radio.SetActive(option.Mode == main.RoutingMode);
            updating = false;
        }, nameof(main.RoutingMode));
        // The team's routing rules on the web (they apply to all its devices).
        var rulesRow = Row(T("editRoutingRules"), T("editRoutingRulesD"));
        var openRules = Gtk.Button.NewFromIconName("go-next-symbolic");
        openRules.SetValign(Gtk.Align.Center);
        openRules.AddCssClass("flat");
        openRules.OnClicked += (_, _) => vm.OpenRoutingRulesCommand.Execute();
        rulesRow.AddSuffix(openRules);
        rulesRow.SetActivatableWidget(openRules);
        routing.Add(rulesRow);
        page.Add(routing);

        // API override (always shown; development only).
        var api = Group(T("apiOverride"), T("apiHint"));
        var apiRow = Row(T("apiOverride"));
        var entry = Gtk.Entry.New();
        entry.SetPlaceholderText(vm.ApiPlaceholder);
        entry.SetText(vm.ApiBaseOverride);
        entry.AddCssClass("ppvpn-mono");
        entry.SetValign(Gtk.Align.Center);
        entry.SetHexpand(true);
        entry.OnChanged += (_, _) => vm.ApiBaseOverride = entry.GetText();
        apiRow.AddSuffix(entry);
        api.Add(apiRow);
        page.Add(api);

        // The Enhanced Mode system service.
        var service = Group(T("service"));
        var (status, statusValue) = ValueRow(T("status"));
        var install = TextButton(T("installBtn"), () => main.InstallServiceCommand.Execute());
        var uninstall = TextButton(T("uninstallBtn"), () => main.UninstallServiceCommand.Execute(), "destructive-action");
        status.AddSuffix(install);
        status.AddSuffix(uninstall);
        service.Add(status);
        main.Bind(() =>
        {
            statusValue.SetText(main.ServiceStatusText);
            install.SetVisible(!main.ServiceInstalled);
            uninstall.SetVisible(main.ServiceInstalled);
            install.SetSensitive(!main.IsInstallingService);
        }, nameof(main.ServiceStatusText), nameof(main.ServiceInstalled), nameof(main.IsInstallingService));
        page.Add(service);
        return page;
    }
}
