using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// Connection card (status header + Connect switch, method caption, install hint, current node),
/// notices, live traffic, the current node's local proxy; or the restricted empty state.
/// </summary>
public static class OverviewPage
{
    /// <param name="openAccountMenu">"Switch team" in the restricted state pops the account menu.</param>
    public static Gtk.Widget Create(MainViewModel vm, Action openAccountMenu)
    {
        var stack = Gtk.Stack.New();
        stack.AddNamed(Restricted(vm, openAccountMenu), "restricted");

        var page = Gtk.Box.New(Gtk.Orientation.Vertical, 24);
        page.Append(ConnectionCard(vm));
        page.Append(Notices(vm));
        page.Append(Traffic(vm));
        page.Append(LocalProxy(vm));
        stack.AddNamed(Clamped(page), "connection");

        vm.Bind(() => stack.SetVisibleChildName(vm.IsRestricted ? "restricted" : "connection"), nameof(vm.IsRestricted));
        return stack;
    }

    private static Gtk.Widget Restricted(MainViewModel vm, Action openAccountMenu)
    {
        var status = Adw.StatusPage.New();
        status.SetIconName("dialog-warning-symbolic");
        var buttons = Gtk.Box.New(Gtk.Orientation.Horizontal, 12);
        buttons.SetHalign(Gtk.Align.Center);
        var refresh = TextButton(T("refresh"), () => vm.RefreshAccessCommand.Execute(), "pill");
        var buy = TextButton(T("buy") + " ↗", () => vm.OpenPurchaseCommand.Execute(), "pill", "suggested-action");
        var switchTeam = TextButton(T("switchTeam") + " ▾", openAccountMenu, "pill", "suggested-action");
        buttons.Append(refresh);
        buttons.Append(buy);
        buttons.Append(switchTeam);
        status.SetChild(buttons);
        vm.Bind(() =>
        {
            status.SetTitle(vm.RestrictedTitle);
            status.SetDescription(vm.RestrictedMessage);
            buy.SetVisible(vm.CanBuy);
            switchTeam.SetVisible(vm.CanSwitchTeam);
            refresh.SetSensitive(!vm.IsRefreshingAccess);
            refresh.SetLabel(vm.IsRefreshingAccess ? "…" : T("refresh"));
        }, nameof(vm.RestrictedTitle), nameof(vm.RestrictedMessage), nameof(vm.CanBuy), nameof(vm.CanSwitchTeam),
            nameof(vm.IsRefreshingAccess));
        return status;
    }

    private static Gtk.Widget ConnectionCard(MainViewModel vm)
    {
        var card = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        card.AddCssClass("ppvpn-card");

        // Header: status circle · title / node line · Connect switch.
        var header = Gtk.Box.New(Gtk.Orientation.Horizontal, 14);
        header.AddCssClass("ppvpn-card-header");
        var circle = Gtk.Overlay.New();
        var circleBox = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        circleBox.AddCssClass("ppvpn-status-circle");
        var statusIcon = Gtk.Image.New();
        statusIcon.SetPixelSize(22);
        statusIcon.SetVexpand(true);
        statusIcon.SetValign(Gtk.Align.Center);
        circleBox.Append(statusIcon);
        var circleSpinner = Gtk.Spinner.New();
        circleSpinner.SetSizeRequest(22, 22);
        circleSpinner.SetHalign(Gtk.Align.Center);
        circleSpinner.SetValign(Gtk.Align.Center);
        circle.SetChild(circleBox);
        circle.AddOverlay(circleSpinner);
        circle.SetValign(Gtk.Align.Center);

        var text = Gtk.Box.New(Gtk.Orientation.Vertical, 2);
        text.SetHexpand(true);
        text.SetValign(Gtk.Align.Center);
        var title = Label(null, "ppvpn-status-title");
        title.SetEllipsize(Pango.EllipsizeMode.End);
        var detailLine = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        var detailFlag = Flag(null);
        var detail = Label(null, "dim-label");
        detail.SetEllipsize(Pango.EllipsizeMode.End);
        detailLine.Append(detailFlag);
        detailLine.Append(detail);
        text.Append(title);
        text.Append(detailLine);

        // Three states: off, on, and in between (spinner over a centred knob, soft track).
        var connect = new BoundSwitch(_ => vm.ToggleConnectCommand.Execute());
        connect.Switch.SetTooltipText(T("connect"));
        var switchOverlay = Gtk.Overlay.New();
        switchOverlay.SetChild(connect.Switch);
        switchOverlay.SetValign(Gtk.Align.Center);
        var switchSpinner = Gtk.Spinner.New();
        switchSpinner.SetSizeRequest(14, 14);
        switchSpinner.SetHalign(Gtk.Align.Center);
        switchSpinner.SetValign(Gtk.Align.Center);
        switchSpinner.SetCanTarget(false);
        switchOverlay.AddOverlay(switchSpinner);

        header.Append(circle);
        header.Append(text);
        header.Append(switchOverlay);
        card.Append(header);

        var caption = Label(null, "ppvpn-weakest", "ppvpn-caption");
        card.Append(caption);

        // Enhanced mode without the system service: a standing hint with the install button.
        var hint = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        hint.AddCssClass("ppvpn-install-hint");
        var hintText = Label(T("needInstall"));
        hintText.SetHexpand(true);
        hintText.SetWrap(true);
        var installing = Gtk.Spinner.New();
        var install = TextButton(T("installBtn"), () => vm.InstallServiceCommand.Execute());
        hint.Append(Gtk.Image.NewFromIconName("dialog-password-symbolic"));
        hint.Append(hintText);
        hint.Append(installing);
        hint.Append(install);
        card.Append(hint);

        // Current node.
        var nodes = Gtk.ListBox.New();
        nodes.SetSelectionMode(Gtk.SelectionMode.None);
        nodes.AddCssClass("boxed-list");
        nodes.SetMarginStart(12);
        nodes.SetMarginEnd(12);
        nodes.SetMarginBottom(12);
        var node = new BoundCombo(T("currentNode"), index =>
        {
            if (index < vm.Nodes.Items.Count) vm.CurrentNode = vm.Nodes.Items[index];
        }, flags: true);
        nodes.Append(node.Row);
        card.Append(nodes);

        vm.Bind(() =>
        {
            var tone = Theme.ToneClass(vm.StatusDotTone);
            circleBox.SetToneClass(tone);
            statusIcon.SetToneClass(tone);
            statusIcon.SetFromIconName(vm.StatusDotTone switch
            {
                ConnectionTone.Ok => "security-high-symbolic",
                ConnectionTone.Warn => "dialog-warning-symbolic",
                ConnectionTone.Error => "dialog-error-symbolic",
                ConnectionTone.Busy => null,
                _ => "network-offline-symbolic",
            });
            circleSpinner.SetVisible(vm.IsConnectionBusy);
            circleSpinner.SetSpinning(vm.IsConnectionBusy);
            title.SetText(vm.ConnectionTitle);
            if (vm.IsConnectionError) title.SetToneClass("tone-error");
            else title.RemoveCssClass("tone-error");
            var parts = new List<string>();
            detailFlag.SetVisible(SetFlag(detailFlag, vm.CurrentNodeName.Length > 0 ? vm.CurrentNodeCountryCode : null));
            if (vm.CurrentNodeName.Length > 0) parts.Add(vm.CurrentNodeName);
            if (vm.ConnectionDetail.Length > 0) parts.Add(vm.ConnectionDetail);
            detail.SetText(string.Join(" · ", parts));
            detailLine.SetVisible(parts.Count > 0);

            connect.Set(vm.ConnectSwitch == SwitchVisual.On);
            var busy = vm.ConnectSwitch == SwitchVisual.Indeterminate;
            if (busy) connect.Switch.AddCssClass("ppvpn-busy");
            else connect.Switch.RemoveCssClass("ppvpn-busy");
            switchSpinner.SetVisible(busy);
            switchSpinner.SetSpinning(busy);
            connect.Switch.SetSensitive(vm.ConnectSwitchEnabled);

            caption.SetText(vm.ConnectionMethodCaption);
            hint.SetVisible(vm.ShowInstallHint);
            installing.SetVisible(vm.IsInstallingService);
            installing.SetSpinning(vm.IsInstallingService);
            install.SetSensitive(!vm.IsInstallingService);
        }, nameof(vm.StatusDotTone), nameof(vm.IsConnectionBusy), nameof(vm.IsConnectionError), nameof(vm.ConnectionTitle),
            nameof(vm.ConnectionDetail), nameof(vm.CurrentNodeName), nameof(vm.CurrentNodeCountryCode), nameof(vm.ConnectSwitch),
            nameof(vm.ConnectSwitchEnabled), nameof(vm.ConnectionMethodCaption), nameof(vm.ShowInstallHint),
            nameof(vm.IsInstallingService), nameof(vm.ConnectState));

        string NodeTitle(NodeItemViewModel item)
        {
            return item.LatencyText.Length > 0 ? $"{item.Name} · {item.LatencyText}" : item.Name;
        }
        void RenderNodes()
        {
            node.SetItems(vm.Nodes.Items.Select(NodeTitle), vm.CurrentNode is { } current ? vm.Nodes.Items.IndexOf(current) : -1,
                vm.Nodes.Items.Select(item => (string?)item.CountryCode).ToList());
            node.Row.SetSensitive(vm.Nodes.Items.Count > 0);
        }
        vm.Nodes.Items.BindItems(RenderNodes);
        vm.Bind(() => node.Select(vm.CurrentNode is { } current ? vm.Nodes.Items.IndexOf(current) : -1),
            nameof(vm.CurrentNode));
        // Latencies are part of the titles: refresh them when a probe run ends.
        vm.Nodes.Bind(RenderNodes, "IsProbing");
        return card;
    }

    private static Gtk.Widget Notices(MainViewModel vm)
    {
        var list = Gtk.Box.New(Gtk.Orientation.Vertical, 12);
        vm.Notices.BindItems(() =>
        {
            while (list.GetFirstChild() is { } child) list.Remove(child);
            foreach (var notice in vm.Notices) list.Append(NoticeCard(notice));
            list.SetVisible(vm.Notices.Count > 0);
        });
        return list;
    }

    private static Gtk.Widget NoticeCard(ConnectionNotice notice)
    {
        var card = Gtk.Box.New(Gtk.Orientation.Horizontal, 12);
        card.AddCssClass("ppvpn-notice");
        card.SetToneClass(Theme.ToneClass(notice.Tone));
        var icon = Gtk.Image.NewFromIconName(notice.Kind switch
        {
            NoticeKind.Occupied => "computer-symbolic",
            // Another VPN or proxy tool has the network.
            NoticeKind.Conflict => "network-vpn-symbolic",
            // Some routing rules have not loaded yet: a warning, the connection works.
            NoticeKind.RulesUnavailable => "dialog-warning-symbolic",
            // The pinned line is down, or a profile refresh dropped a pinned line.
            NoticeKind.IngressUnavailable or NoticeKind.IngressPinCleared => "dialog-warning-symbolic",
            // The local proxy got new credentials: apps using it must copy them again.
            NoticeKind.LocalProxyCredentialsReset => "dialog-password-symbolic",
            // Enhanced mode took another VPN's routes over for now (macOS; not shown on Linux).
            NoticeKind.RoutesReplaced => "dialog-information-symbolic",
            _ => "dialog-error-symbolic",
        });
        icon.SetValign(Gtk.Align.Start);
        icon.SetToneClass(Theme.ToneClass(notice.Tone));
        var text = Gtk.Box.New(Gtk.Orientation.Vertical, 2);
        text.SetHexpand(true);
        var title = Label(notice.Title, "ppvpn-notice-title");
        title.SetWrap(true);
        var message = Label(notice.Message, "dim-label");
        message.SetWrap(true);
        text.Append(title);
        text.Append(message);
        card.Append(icon);
        card.Append(text);
        if (notice.SecondaryActionText is { } secondaryText && notice.SecondaryAction is { } secondary)
            card.Append(TextButton(secondaryText, () => secondary.Execute()));
        if (notice.ActionText is { } actionText && notice.Action is { } action)
            card.Append(TextButton(actionText, () => action.Execute(), "suggested-action"));
        return card;
    }

    private static Gtk.Widget Traffic(MainViewModel vm)
    {
        var group = Group(T("traffic"));
        var tiles = Gtk.Box.New(Gtk.Orientation.Horizontal, 12);
        tiles.SetHomogeneous(true);
        Gtk.Label Tile(string key, string icon)
        {
            var tile = Gtk.Box.New(Gtk.Orientation.Vertical, 4);
            tile.AddCssClass("ppvpn-card");
            var head = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
            head.SetMarginTop(12);
            head.SetMarginStart(14);
            head.Append(Gtk.Image.NewFromIconName(icon));
            head.Append(Label(T(key), "dim-label"));
            var rate = Label("—", "ppvpn-rate");
            rate.SetMarginStart(14);
            rate.SetMarginBottom(12);
            tile.Append(head);
            tile.Append(rate);
            tiles.Append(tile);
            return rate;
        }
        var up = Tile("upload", "go-up-symbolic");
        var down = Tile("download", "go-down-symbolic");
        group.Add(tiles);
        vm.Bind(() =>
        {
            group.SetVisible(vm.ShowTraffic);
            up.SetText(vm.UpRate);
            down.SetText(vm.DownRate);
        }, nameof(vm.ShowTraffic), nameof(vm.UpRate), nameof(vm.DownRate));
        return group;
    }

    private static Gtk.Widget LocalProxy(MainViewModel vm)
    {
        // The note follows the shown user (routed: by the rules; node: this node on its own).
        var group = Group(T("localProxy"), vm.ProxyNote);
        // Which user the card shows: follow the routing rules (default) or this node only. Hidden
        // while the core serves no routed user. The choices are fixed, so the model is set once.
        var scope = Gtk.DropDown.NewFromStrings(vm.ProxyScopes.Select(o => o.Title).ToArray());
        scope.SetValign(Gtk.Align.Center);
        scope.SetListFactory(ScopeFactory(vm.ProxyScopes));
        var updatingScope = false;
        scope.OnNotify += (_, args) =>
        {
            if (updatingScope || args.Pspec.GetName() != "selected") return;
            var index = (int)scope.GetSelected();
            if (index >= 0 && index < vm.ProxyScopes.Count) vm.SelectedProxyScope = vm.ProxyScopes[index];
        };
        group.SetHeaderSuffix(scope);
        var (http, httpValue) = ValueRow("HTTP", monospace: true);
        http.AddSuffix(CopyButton(() => vm.ShownProxy?.CopyHttpCommand));
        var (socks, socksValue) = ValueRow("SOCKS5", monospace: true);
        socks.AddSuffix(CopyButton(() => vm.ShownProxy?.CopySocksCommand));
        var (user, password, refreshCredentials) = CredentialRows(() => vm.ShownProxy);
        var unavailable = Row("");
        unavailable.AddCssClass("dim-label");
        group.Add(http);
        group.Add(socks);
        group.Add(user);
        group.Add(password);
        group.Add(unavailable);
        vm.Bind(() =>
        {
            scope.SetVisible(vm.HasRoutedProxy);
            var index = Math.Max(0, vm.ProxyScopes.ToList().IndexOf(vm.SelectedProxyScope));
            if (scope.GetSelected() != (uint)index)
            {
                updatingScope = true;
                scope.SetSelected((uint)index);
                updatingScope = false;
            }
        }, nameof(vm.HasRoutedProxy), nameof(vm.SelectedProxyScope));
        vm.Bind(() =>
        {
            var proxy = vm.ShownProxy;
            http.SetVisible(proxy is not null);
            socks.SetVisible(proxy is not null);
            user.SetVisible(proxy is not null);
            password.SetVisible(proxy is not null);
            unavailable.SetVisible(proxy is null);
            httpValue.SetText(proxy?.HttpDisplay ?? "");
            socksValue.SetText(proxy?.SocksDisplay ?? "");
            refreshCredentials();
            unavailable.SetTitle(vm.ProxyUnavailableText ?? "");
            group.SetVisible(proxy is not null || vm.ProxyUnavailableText is not null);
            group.SetDescription(vm.ProxyNote);
        }, nameof(vm.ShownProxy), nameof(vm.HasShownProxy), nameof(vm.ProxyUnavailableText), nameof(vm.ProxyNote));
        return group;
    }

    /// <summary>The proxy user picker's list rows: the choice, its description in small type below.</summary>
    private static Gtk.SignalListItemFactory ScopeFactory(IReadOnlyList<LocalProxyScopeOption> options)
    {
        var factory = Gtk.SignalListItemFactory.New();
        factory.OnSetup += (_, args) =>
        {
            var row = (Gtk.ListItem)args.Object;
            var text = Gtk.Box.New(Gtk.Orientation.Vertical, 2);
            text.SetHexpand(true);
            var title = Label(null);
            title.SetXalign(0);
            var description = Label(null, "dim-label", "caption");
            description.SetXalign(0);
            description.SetWrap(true);
            // Wide enough for a description to read as one or two lines, not a narrow column.
            description.SetWidthChars(34);
            description.SetMaxWidthChars(40);
            text.Append(title);
            text.Append(description);
            var check = Gtk.Image.NewFromIconName("object-select-symbolic");
            check.SetValign(Gtk.Align.Center);
            row.OnNotify += (_, notify) =>
            {
                if (notify.Pspec.GetName() == "selected") check.SetOpacity(row.GetSelected() ? 1 : 0);
            };
            var box = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
            box.Append(text);
            box.Append(check);
            row.SetChild(box);
        };
        factory.OnBind += (_, args) =>
        {
            var row = (Gtk.ListItem)args.Object;
            var value = ((Gtk.StringObject)row.GetItem()!).GetString();
            var box = (Gtk.Box)row.GetChild()!;
            var text = (Gtk.Box)box.GetFirstChild()!;
            ((Gtk.Label)text.GetFirstChild()!).SetText(value);
            ((Gtk.Label)text.GetLastChild()!).SetText(options.FirstOrDefault(o => o.Title == value)?.Description ?? "");
            box.GetLastChild()!.SetOpacity(row.GetSelected() ? 1 : 0);
        };
        return factory;
    }
}
