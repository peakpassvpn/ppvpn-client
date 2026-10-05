using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// Node table in a card (select a row, double-click to use it, right-click for more), the
/// probe toolbar at the top of the content, and the selected node's local proxy at the bottom.
/// </summary>
public static class NodesPage
{
    public static Gtk.Widget Create(NodesViewModel vm)
    {
        var root = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        root.Append(Toolbar(vm));

        var stack = Gtk.Stack.New();
        stack.SetVexpand(true);
        stack.AddNamed(Status("content-loading-symbolic", "loadingT", "loadingD", spinner: true), NodesViewState.Loading.ToString());
        var invalid = Status("dialog-warning-symbolic", "invalidT", "invalidD");
        invalid.SetChild(PillButton(T("refreshNodes"), () => vm.RefreshCommand.Execute()));
        stack.AddNamed(invalid, NodesViewState.InvalidNoHistory.ToString());
        var restricted = Adw.StatusPage.New();
        restricted.SetIconName("dialog-warning-symbolic");
        restricted.SetChild(PillButton(T("refresh"), () => vm.Main.RefreshAccessCommand.Execute()));
        stack.AddNamed(restricted, NodesViewState.Restricted.ToString());
        stack.AddNamed(Table(vm), NodesViewState.Data.ToString());
        root.Append(stack);

        vm.Bind(() => stack.SetVisibleChildName(vm.ViewState.ToString()), nameof(vm.ViewState));
        vm.Main.Bind(() =>
        {
            restricted.SetTitle(vm.Main.RestrictedTitle);
            restricted.SetDescription(vm.Main.RestrictedMessage);
        }, nameof(vm.Main.RestrictedTitle), nameof(vm.Main.RestrictedMessage));
        return root;
    }

    private static Gtk.Widget Toolbar(NodesViewModel vm)
    {
        // The design's 46px bar at the top of the content (never in the header bar).
        var bar = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        bar.SetSizeRequest(-1, 46);
        bar.SetMarginStart(12);
        bar.SetMarginEnd(12);
        bar.Append(Label(T("probe"), "dim-label"));
        var methods = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        methods.AddCssClass("linked");
        methods.SetValign(Gtk.Align.Center);
        var toggles = new List<Gtk.ToggleButton>();
        var updating = false;
        foreach (var (label, index) in vm.MethodLabelsList.Select((label, index) => (label, index)))
        {
            var toggle = Gtk.ToggleButton.NewWithLabel(label);
            if (toggles.Count > 0) toggle.SetGroup(toggles[0]);
            toggle.OnToggled += (_, _) =>
            {
                if (!updating && toggle.GetActive()) vm.MethodIndex = index;
            };
            toggles.Add(toggle);
            methods.Append(toggle);
        }
        bar.Append(methods);
        var spacer = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        spacer.SetHexpand(true);
        bar.Append(spacer);
        var testing = Gtk.Spinner.New();
        var testingText = Label(null, "dim-label");
        bar.Append(testing);
        bar.Append(testingText);
        var testAll = TextButton(T("testAll"), () => vm.TestAllCommand.Execute());
        bar.Append(testAll);
        bar.Append(IconButton("view-refresh-symbolic", T("refreshNodes"), () => vm.RefreshCommand.Execute()));
        vm.Bind(() =>
        {
            updating = true;
            if (vm.MethodIndex >= 0 && vm.MethodIndex < toggles.Count) toggles[vm.MethodIndex].SetActive(true);
            updating = false;
            methods.SetSensitive(vm.CanProbe);
            testAll.SetSensitive(vm.CanProbe && vm.Items.Count > 0 && !vm.IsProbing);
            testing.SetVisible(vm.IsProbing);
            testing.SetSpinning(vm.IsProbing);
            testingText.SetVisible(vm.IsProbing);
            testingText.SetText(vm.TestingText);
        }, nameof(vm.MethodIndex), nameof(vm.CanProbe), nameof(vm.IsProbing), nameof(vm.TestingText), nameof(vm.ViewState));
        return bar;
    }

    private static Gtk.Widget Table(NodesViewModel vm)
    {
        var page = Gtk.Box.New(Gtk.Orientation.Vertical, 12);
        var sizes = new Columns();

        var card = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        card.AddCssClass("ppvpn-card");
        card.SetOverflow(Gtk.Overflow.Hidden);
        card.Append(HeaderRow(sizes));
        card.Append(Gtk.Separator.New(Gtk.Orientation.Horizontal));
        var list = Gtk.ListBox.New();
        list.SetSelectionMode(Gtk.SelectionMode.Single);
        list.SetActivateOnSingleClick(false);
        list.AddCssClass("ppvpn-node-list");
        card.Append(list);
        page.Append(card);
        page.Append(SelectedProxyCard(vm));

        var rows = new Dictionary<Gtk.ListBoxRow, NodeItemViewModel>();
        var subscriptions = new SubscriptionBag();
        var selecting = false;
        vm.Items.BindItems(() =>
        {
            subscriptions.Dispose();
            rows.Clear();
            while (list.GetFirstChild() is { } child) list.Remove(child);
            foreach (var item in vm.Items)
            {
                var row = NodeRow(item, sizes, subscriptions);
                rows[row] = item;
                list.Append(row);
            }
            SyncSelection();
        });
        void SyncSelection()
        {
            selecting = true;
            var row = rows.FirstOrDefault(pair => pair.Value == vm.SelectedItem).Key;
            if (row is null) list.UnselectAll();
            else list.SelectRow(row);
            selecting = false;
        }
        vm.Bind(SyncSelection, "SelectedItem");
        list.OnRowSelected += (_, args) =>
        {
            if (selecting || args.Row is not { } row || !rows.TryGetValue(row, out var item)) return;
            vm.SelectedItem = item;
        };
        list.OnRowActivated += (_, args) =>
        {
            if (rows.TryGetValue(args.Row, out var item)) item.SetCurrentCommand.Execute();
        };
        return Clamped(page);
    }

    /// <summary>Column widths shared by the header and every row.</summary>
    private sealed class Columns
    {
        public readonly Gtk.SizeGroup Mark = Gtk.SizeGroup.New(Gtk.SizeGroupMode.Horizontal);
        public readonly Gtk.SizeGroup Tier = Gtk.SizeGroup.New(Gtk.SizeGroupMode.Horizontal);
        public readonly Gtk.SizeGroup Region = Gtk.SizeGroup.New(Gtk.SizeGroupMode.Horizontal);
        public readonly Gtk.SizeGroup Routes = Gtk.SizeGroup.New(Gtk.SizeGroupMode.Horizontal);
        public readonly Gtk.SizeGroup Latency = Gtk.SizeGroup.New(Gtk.SizeGroupMode.Horizontal);
    }

    private static Gtk.Box RowBox()
    {
        var box = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        box.SetMarginStart(12);
        box.SetMarginEnd(12);
        box.SetSizeRequest(-1, 40);
        return box;
    }

    private static Gtk.Widget HeaderRow(Columns columns)
    {
        var box = RowBox();
        box.AddCssClass("ppvpn-table-header");
        Gtk.Label Cell(string key, Gtk.SizeGroup? group, bool expand = false, float xalign = 0)
        {
            var label = Label(key.Length > 0 ? T(key) : "");
            label.SetXalign(xalign);
            label.SetHexpand(expand);
            group?.AddWidget(label);
            box.Append(label);
            return label;
        }
        Cell("", columns.Mark).SetSizeRequest(16, -1);
        Cell("colName", null, expand: true);
        Cell("colTier", columns.Tier);
        Cell("colRegion", columns.Region);
        Cell("colRoutes", columns.Routes);
        Cell("colLatency", columns.Latency, xalign: 1);
        return box;
    }

    private static Gtk.ListBoxRow NodeRow(NodeItemViewModel item, Columns columns, SubscriptionBag subscriptions)
    {
        var row = Gtk.ListBoxRow.New();
        var box = RowBox();
        var mark = Gtk.Image.NewFromIconName("object-select-symbolic");
        mark.AddCssClass("tone-busy");
        mark.SetSizeRequest(16, -1);
        columns.Mark.AddWidget(mark);
        var nameBox = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        nameBox.SetHexpand(true);
        nameBox.Append(Flag(item.CountryCode));
        var name = Label(item.Name);
        name.SetEllipsize(Pango.EllipsizeMode.End);
        nameBox.Append(name);
        var tierBox = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        var tier = Label(item.Tier, "ppvpn-tag");
        tier.SetValign(Gtk.Align.Center);
        tierBox.Append(tier);
        columns.Tier.AddWidget(tierBox);
        var region = Label(item.Region, "dim-label");
        columns.Region.AddWidget(region);
        // Routes: the line names, or for a node with more than one line a picker (automatic
        // failover, or pinned to one line) with a pin mark.
        var routesBox = Gtk.Box.New(Gtk.Orientation.Horizontal, 4);
        var routes = Label(item.Routes, "dim-label");
        routes.SetEllipsize(Pango.EllipsizeMode.End);
        routes.SetMaxWidthChars(28);
        var lines = Gtk.DropDown.NewFromStrings([]);
        lines.SetValign(Gtk.Align.Center);
        lines.AddCssClass("flat");
        // The button shows the chosen line ellipsized, so a long name does not widen the column
        // (and squeeze the node names); the list shows the full names, the line carrying the
        // node's latest connection with a dot.
        string? activeLine = null;
        string[] lineTitles = [];
        lines.SetFactory(LineFactory(ellipsize: true, () => null));
        lines.SetListFactory(LineFactory(ellipsize: false, () => activeLine));
        var updatingLines = false;
        lines.OnNotify += (_, args) =>
        {
            if (updatingLines || args.Pspec.GetName() != "selected") return;
            var index = (int)lines.GetSelected();
            var options = item.Lines;
            if (index >= 0 && index < options.Count) item.SelectedLine = options[index];
        };
        var pin = Gtk.Image.New();
        pin.SetValign(Gtk.Align.Center);
        routesBox.Append(routes);
        routesBox.Append(lines);
        routesBox.Append(pin);
        columns.Routes.AddWidget(routesBox);
        var latencyBox = Gtk.Box.New(Gtk.Orientation.Horizontal, 4);
        latencyBox.SetHalign(Gtk.Align.End);
        var latencySpinner = Gtk.Spinner.New();
        var latencyIcon = Gtk.Image.New();
        var latency = Label(null, "numeric");
        latency.SetXalign(1);
        latencyBox.Append(latencySpinner);
        latencyBox.Append(latencyIcon);
        latencyBox.Append(latency);
        columns.Latency.AddWidget(latencyBox);

        box.Append(mark);
        box.Append(nameBox);
        box.Append(tierBox);
        box.Append(region);
        box.Append(routesBox);
        box.Append(latencyBox);
        row.SetChild(box);

        var menu = ContextMenu(item);
        menu.SetParent(row);
        var rightClick = Gtk.GestureClick.New();
        rightClick.SetButton(3);
        rightClick.OnPressed += (_, args) =>
        {
            menu.SetPointingTo(new Gdk.Rectangle { X = (int)args.X, Y = (int)args.Y, Width = 1, Height = 1 });
            menu.Popup();
        };
        row.AddController(rightClick);

        subscriptions.Add(item.Bind(() =>
        {
            mark.SetOpacity(item.IsCurrent ? 1 : 0);
            if (item.IsCurrent) name.AddCssClass("ppvpn-current");
            else name.RemoveCssClass("ppvpn-current");
            tier.SetVisible(item.HasTier);
            tier.SetText(item.Tier);
            latencySpinner.SetVisible(item.IsTesting);
            latencySpinner.SetSpinning(item.IsTesting);
            latency.SetVisible(!item.IsTesting);
            latency.SetText(item.LatencyText);
            latency.SetToneClass(Theme.ToneClass(item.LatencyTone));
            latency.SetTooltipText(item.LatencyTooltip);
            // Timeout: secondary text with a clock; failed: danger with an error icon.
            var icon = item.LatencyKind switch
            {
                LatencyKind.Timeout => "alarm-symbolic",
                LatencyKind.Failed => "dialog-error-symbolic",
                _ => null,
            };
            latencyIcon.SetVisible(icon is not null && !item.IsTesting);
            if (icon is not null) latencyIcon.SetFromIconName(icon);
            latencyIcon.SetToneClass(Theme.ToneClass(item.LatencyTone));
        }, nameof(item.IsCurrent), nameof(item.LatencyText), nameof(item.LatencyTone), nameof(item.LatencyTooltip),
            nameof(item.Node), nameof(item.LatencyKind)));
        subscriptions.Add(item.Bind(() =>
        {
            var options = item.Lines;
            routes.SetText(item.Routes);
            routes.SetVisible(!item.HasLineChoice);
            lines.SetVisible(item.HasLineChoice);
            updatingLines = true;
            // A new model only when the lines or the dot change: replacing it while the picker
            // is delivering a choice (SelectedLine set from its "selected" notify) frees the list
            // rows under it and crashes.
            var titles = options.Select(o => o.Title).ToArray();
            var active = options.FirstOrDefault(o => o.EndpointKey is not null && o.EndpointKey == item.ActiveEndpointKey)?.Title;
            if (!titles.SequenceEqual(lineTitles) || active != activeLine)
            {
                lineTitles = titles;
                activeLine = active;
                lines.SetModel(Gtk.StringList.New(titles));
            }
            var index = Math.Max(0, options.ToList().IndexOf(item.SelectedLine ?? options[0]));
            if (lines.GetSelected() != (uint)index) lines.SetSelected((uint)index);
            updatingLines = false;
            // Pinned: a pin; pinned to a line the core reports down: a warning (it stays pinned).
            pin.SetVisible(item.HasLineChoice && item.IsPinned);
            pin.SetFromIconName(item.PinnedUnavailable ? "dialog-warning-symbolic" : "view-pin-symbolic");
            pin.SetToneClass(item.PinnedUnavailable ? "tone-warn" : "tone-busy");
        }, nameof(item.Node), nameof(item.SelectedLine), nameof(item.IsPinned), nameof(item.PinnedUnavailable),
            nameof(item.ActiveEndpointKey)));
        return row;
    }

    /// <summary>
    /// Line picker rows: the title ellipsized (the button), or in full with a dot on
    /// <paramref name="marked"/> and a check on the chosen line (the list).
    /// </summary>
    private static Gtk.SignalListItemFactory LineFactory(bool ellipsize, Func<string?> marked)
    {
        var factory = Gtk.SignalListItemFactory.New();
        factory.OnSetup += (_, args) =>
        {
            var row = (Gtk.ListItem)args.Object;
            var box = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
            var label = Gtk.Label.New(null);
            label.SetXalign(0);
            box.Append(label);
            if (ellipsize)
            {
                // In the middle: line names often share a prefix ("Hangzhou Test" / "… Backup").
                label.SetEllipsize(Pango.EllipsizeMode.Middle);
                label.SetMaxWidthChars(11);
            }
            else
            {
                var check = Gtk.Image.NewFromIconName("object-select-symbolic");
                box.Append(check);
                row.OnNotify += (_, notify) =>
                {
                    if (notify.Pspec.GetName() == "selected") check.SetOpacity(row.GetSelected() ? 1 : 0);
                };
            }
            row.SetChild(box);
        };
        factory.OnBind += (_, args) =>
        {
            var row = (Gtk.ListItem)args.Object;
            var box = (Gtk.Box)row.GetChild()!;
            var title = ((Gtk.StringObject)row.GetItem()!).GetString();
            ((Gtk.Label)box.GetFirstChild()!).SetText(!ellipsize && title == marked() ? $"{title} •" : title);
            if (!ellipsize) box.GetLastChild()!.SetOpacity(row.GetSelected() ? 1 : 0);
        };
        return factory;
    }

    private static Gtk.Popover ContextMenu(NodeItemViewModel item)
    {
        var popover = Gtk.Popover.New();
        popover.SetHasArrow(false);
        popover.SetPosition(Gtk.PositionType.Bottom);
        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        Gtk.Button Item(string key, Action run)
        {
            var button = Gtk.Button.NewWithLabel(T(key));
            button.AddCssClass("flat");
            ((Gtk.Label)button.GetChild()!).SetXalign(0);
            button.OnClicked += (_, _) =>
            {
                popover.Popdown();
                run();
            };
            box.Append(button);
            return button;
        }
        var setCurrent = Item("setCurrent", () => item.SetCurrentCommand.Execute());
        Item("testOne", () => item.TestOneCommand.Execute());
        box.Append(Gtk.Separator.New(Gtk.Orientation.Horizontal));
        var copyHttp = Item("copyHttp", () => item.CopyHttpCommand.Execute());
        var copySocks = Item("copySocks", () => item.CopySocksCommand.Execute());
        popover.SetChild(box);
        popover.OnShow += (_, _) =>
        {
            setCurrent.SetSensitive(!item.IsCurrent);
            copyHttp.SetSensitive(item.HasProxy);
            copySocks.SetSensitive(item.HasProxy);
        };
        return popover;
    }

    private static Gtk.Widget SelectedProxyCard(NodesViewModel vm)
    {
        var group = Group();
        var (http, httpValue) = ValueRow("HTTP", monospace: true);
        http.AddSuffix(CopyButton(() => vm.SelectedNodeProxy?.CopyHttpCommand));
        var (socks, socksValue) = ValueRow("SOCKS5", monospace: true);
        socks.AddSuffix(CopyButton(() => vm.SelectedNodeProxy?.CopySocksCommand));
        var (user, password, refreshCredentials) = CredentialRows(() => vm.SelectedNodeProxy);
        group.Add(http);
        group.Add(socks);
        group.Add(user);
        group.Add(password);
        var footer = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        var count = Label(null, "ppvpn-weakest");
        var hint = Label(T("nodesHint"), "ppvpn-weakest");
        hint.SetHexpand(true);
        hint.SetXalign(1);
        footer.Append(count);
        footer.Append(hint);
        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 8);
        box.Append(group);
        box.Append(footer);
        vm.Bind(() =>
        {
            var proxy = vm.SelectedNodeProxy;
            group.SetVisible(proxy is not null);
            group.SetTitle(vm.SelectedNodeProxyTitle);
            httpValue.SetText(proxy?.HttpDisplay ?? "");
            socksValue.SetText(proxy?.SocksDisplay ?? "");
            refreshCredentials();
            count.SetText(vm.NodeCountText);
        }, "SelectedItem", nameof(vm.HasSelection), nameof(vm.NodeCountText));
        return box;
    }

    private static Adw.StatusPage Status(string icon, string titleKey, string descriptionKey, bool spinner = false)
    {
        var page = Adw.StatusPage.New();
        page.SetIconName(icon);
        page.SetTitle(T(titleKey));
        page.SetDescription(T(descriptionKey));
        if (spinner)
        {
            var spin = Gtk.Spinner.New();
            spin.SetSizeRequest(32, 32);
            spin.Start();
            page.SetChild(spin);
        }
        return page;
    }

    private static Gtk.Button PillButton(string label, Action clicked)
    {
        var button = TextButton(label, clicked, "pill");
        button.SetHalign(Gtk.Align.Center);
        return button;
    }
}
