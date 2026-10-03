using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// The message center: its own window (380 × 610, transient for the main window), with the paged
/// list and a detail view that replaces it in place.
/// </summary>
public static class MessageCenterWindow
{
    public static Adw.Window Create(InboxViewModel vm, Gtk.Window parent)
    {
        var window = Adw.Window.New();
        window.SetTitle(T("msgCenter"));
        window.SetTransientFor(parent);
        window.SetDefaultSize(380, 610);
        window.SetSizeRequest(340, 360);
        window.SetHideOnClose(true);

        var views = Gtk.Stack.New();
        views.SetTransitionType(Gtk.StackTransitionType.SlideLeftRight);
        views.AddNamed(ListView(vm), "list");
        views.AddNamed(DetailView(vm), "detail");
        window.SetContent(views);
        vm.Bind(() => views.SetVisibleChildName(vm.IsDetailOpen ? "detail" : "list"), nameof(vm.Detail));

        // Esc goes back from a message to the list (which keeps its scroll position).
        var keys = Gtk.EventControllerKey.New();
        keys.OnKeyPressed += (_, args) =>
        {
            if (args.Keyval == Gdk.Constants.KEY_F5)
            {
                vm.RefreshCommand.Execute();
                return true;
            }
            if (args.Keyval != Gdk.Constants.KEY_Escape || !vm.IsDetailOpen) return false;
            vm.BackCommand.Execute();
            return true;
        };
        window.AddController(keys);
        window.OnShow += (_, _) => vm.RefreshCommand.Execute();
        return window;
    }

    private static Gtk.Widget ListView(InboxViewModel vm)
    {
        var root = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        var header = Adw.HeaderBar.New();
        header.SetTitleWidget(Adw.WindowTitle.New(T("msgCenter"), ""));
        root.Append(header);

        var top = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        top.SetMarginStart(14);
        top.SetMarginEnd(10);
        top.SetMarginTop(6);
        top.SetMarginBottom(6);
        var count = Label(null, "dim-label");
        count.SetHexpand(true);
        var markAll = TextButton(T("markAllRead"), () => vm.MarkAllReadCommand.Execute(), "flat");
        top.Append(count);
        top.Append(markAll);
        root.Append(top);

        // Load failure: a banner over the cached list (AdwBanner is libadwaita 1.3).
        var error = Gtk.Revealer.New();
        var errorBar = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        errorBar.AddCssClass("ppvpn-banner");
        errorBar.AddCssClass("error");
        var errorText = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        errorText.SetHexpand(true);
        errorText.Append(Label(T("msgErrT"), "heading"));
        var errorMessage = Label(T("msgErrD"));
        errorMessage.SetWrap(true);
        errorText.Append(errorMessage);
        errorBar.Append(errorText);
        errorBar.Append(TextButton(T("retry"), () => vm.RetryCommand.Execute()));
        error.SetChild(errorBar);
        root.Append(error);

        var states = Gtk.Stack.New();
        states.SetVexpand(true);
        states.AddNamed(Skeleton(), InboxState.FirstLoad.ToString());
        var empty = Adw.StatusPage.New();
        empty.SetIconName("mail-unread-symbolic");
        empty.SetTitle(T("msgEmptyT"));
        empty.SetDescription(T("msgEmptyD"));
        states.AddNamed(empty, InboxState.Empty.ToString());

        var list = Gtk.ListBox.New();
        list.SetSelectionMode(Gtk.SelectionMode.None);
        list.AddCssClass("ppvpn-msg-list");
        var footer = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        footer.SetHalign(Gtk.Align.Center);
        footer.SetMarginTop(10);
        footer.SetMarginBottom(14);
        var loadingMore = Gtk.Spinner.New();
        var footerText = Label(null, "dim-label");
        footer.Append(loadingMore);
        footer.Append(footerText);
        var content = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        content.Append(list);
        content.Append(footer);
        var scroller = Gtk.ScrolledWindow.New();
        scroller.SetPolicy(Gtk.PolicyType.Never, Gtk.PolicyType.Automatic);
        scroller.SetChild(content);
        states.AddNamed(scroller, InboxState.Ready.ToString());
        root.Append(states);

        // The next page loads within 24 px of the bottom.
        var adjustment = scroller.GetVadjustment();
        adjustment.OnValueChanged += (_, _) =>
        {
            if (adjustment.GetValue() + adjustment.GetPageSize() >= adjustment.GetUpper() - 24) vm.LoadMoreCommand.Execute();
        };

        var subscriptions = new SubscriptionBag();
        vm.Messages.BindItems(() =>
        {
            subscriptions.Dispose();
            while (list.GetFirstChild() is { } child) list.Remove(child);
            foreach (var item in vm.Messages) list.Append(MessageRow(item, subscriptions));
        });
        vm.Bind(() =>
        {
            count.SetText(vm.HeaderText);
            markAll.SetSensitive(vm.CanMarkAllRead);
            error.SetRevealChild(vm.HasLoadError);
            states.SetVisibleChildName(vm.IsFirstLoad ? InboxState.FirstLoad.ToString()
                : vm.IsEmpty ? InboxState.Empty.ToString() : InboxState.Ready.ToString());
            loadingMore.SetVisible(vm.IsLoadingMore);
            loadingMore.SetSpinning(vm.IsLoadingMore);
            footerText.SetText(vm.IsLoadingMore ? T("loadingMore") : vm.IsExhausted ? T("noMore") : "");
            footer.SetVisible(vm.IsLoadingMore || vm.IsExhausted);
        }, nameof(vm.HeaderText), nameof(vm.CanMarkAllRead), nameof(vm.HasLoadError), nameof(vm.IsFirstLoad),
            nameof(vm.IsEmpty), nameof(vm.IsReady), nameof(vm.IsLoadingMore), nameof(vm.IsExhausted), nameof(vm.UnreadCount));
        return root;
    }

    /// <summary>First load: six placeholder rows (icon circle and three bars) that breathe.</summary>
    private static Gtk.Widget Skeleton()
    {
        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 18);
        box.SetMarginTop(16);
        box.SetMarginStart(16);
        box.SetMarginEnd(16);
        for (var i = 0; i < 6; i++)
        {
            var row = Gtk.Box.New(Gtk.Orientation.Horizontal, 12);
            var circle = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
            circle.AddCssClass("ppvpn-skeleton");
            circle.SetSizeRequest(32, 32);
            circle.SetValign(Gtk.Align.Start);
            var bars = Gtk.Box.New(Gtk.Orientation.Vertical, 8);
            bars.SetHexpand(true);
            foreach (var width in new[] { 180, 120, 260 })
            {
                var bar = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
                bar.AddCssClass("ppvpn-skeleton");
                bar.SetSizeRequest(width, 10);
                bar.SetHalign(Gtk.Align.Start);
                bars.Append(bar);
            }
            row.Append(circle);
            row.Append(bars);
            box.Append(row);
        }
        Breathe(box);
        return box;
    }

    /// <summary>A 1.4 s opacity pulse while the widget is on screen.</summary>
    private static void Breathe(Gtk.Widget widget)
    {
        var mapped = false;
        widget.OnMap += (_, _) =>
        {
            if (mapped) return;
            mapped = true;
            var step = 0;
            GLib.Functions.TimeoutAdd(GLib.Constants.PRIORITY_DEFAULT, 70, () =>
            {
                step = (step + 1) % 20;
                widget.SetOpacity(0.55 + 0.45 * Math.Abs(10 - step) / 10.0);
                return mapped;
            });
        };
        widget.OnUnmap += (_, _) => mapped = false;
    }

    private static string TypeIcon(MessageCategory category) => category switch
    {
        MessageCategory.SubscriptionExpiring => "alarm-symbolic",
        MessageCategory.SubscriptionExpired => "dialog-warning-symbolic",
        MessageCategory.Billing => "document-edit-symbolic",
        MessageCategory.Order => "emblem-documents-symbolic",
        MessageCategory.Route => "network-wired-symbolic",
        MessageCategory.Announcement => "dialog-information-symbolic",
        _ => "mail-unread-symbolic",
    };

    private static Gtk.Widget MessageRow(InboxItemViewModel item, SubscriptionBag subscriptions)
    {
        var row = Gtk.ListBoxRow.New();
        row.SetActivatable(true);
        var outer = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        var bar = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        bar.AddCssClass("ppvpn-msg-bar");
        bar.SetToneClass(Theme.ToneClass(item.Tone));
        var box = Gtk.Box.New(Gtk.Orientation.Horizontal, 10);
        box.SetMarginTop(12);
        box.SetMarginBottom(12);
        box.SetMarginStart(10);
        box.SetMarginEnd(10);
        box.SetHexpand(true);

        var dot = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        dot.AddCssClass("ppvpn-unread-dot");
        dot.SetValign(Gtk.Align.Start);
        dot.SetMarginTop(12);
        var icon = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        icon.AddCssClass("ppvpn-type-icon");
        icon.SetToneClass(Theme.ToneClass(item.Tone));
        icon.SetValign(Gtk.Align.Start);
        // Centre the glyph in the circle without letting its vexpand propagate to the row.
        icon.SetVexpand(false);
        var image = Gtk.Image.NewFromIconName(TypeIcon(item.Category));
        image.SetVexpand(true);
        icon.Append(image);

        var text = Gtk.Box.New(Gtk.Orientation.Vertical, 3);
        text.SetHexpand(true);
        var titleRow = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        var title = Label(item.Title);
        title.SetHexpand(true);
        title.SetEllipsize(Pango.EllipsizeMode.End);
        var time = Label(null, "ppvpn-weakest");
        titleRow.Append(title);
        titleRow.Append(time);
        var meta = Label(item.MetaText, "dim-label", "caption");
        var body = Label(item.Content, "dim-label");
        body.SetWrap(true);
        body.SetWrapMode(Pango.WrapMode.WordChar);
        body.SetLines(2);
        body.SetEllipsize(Pango.EllipsizeMode.End);
        var actions = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        if (item.HasLink)
        {
            var link = TextButton(T("viewDetails") + " ↗", () => item.OpenLinkCommand.Execute(), "flat");
            link.SetHalign(Gtk.Align.Start);
            actions.Append(link);
        }
        var spacer = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        spacer.SetHexpand(true);
        actions.Append(spacer);
        // "Mark as read" shows on hover, unread rows only.
        var markRead = TextButton(T("markRead"), () => item.MarkReadCommand.Execute(), "flat");
        markRead.SetOpacity(0);
        actions.Append(markRead);
        text.Append(titleRow);
        text.Append(meta);
        text.Append(body);
        text.Append(actions);

        var chevron = Gtk.Image.NewFromIconName("go-next-symbolic");
        chevron.AddCssClass("dim-label");
        chevron.SetValign(Gtk.Align.Center);

        box.Append(dot);
        box.Append(icon);
        box.Append(text);
        box.Append(chevron);
        outer.Append(bar);
        outer.Append(box);
        row.SetChild(outer);

        var hover = Gtk.EventControllerMotion.New();
        hover.OnEnter += (_, _) => markRead.SetOpacity(item.IsUnread ? 1 : 0);
        hover.OnLeave += (_, _) => markRead.SetOpacity(0);
        row.AddController(hover);
        var click = Gtk.GestureClick.New();
        click.OnReleased += (_, args) =>
        {
            if (click.GetCurrentButton() == 1) item.ShowDetailCommand.Execute();
        };
        row.AddController(click);
        var menu = Gtk.Popover.New();
        menu.SetHasArrow(false);
        var menuBox = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        var menuRead = TextButton(T("markRead"), () => { menu.Popdown(); item.MarkReadCommand.Execute(); }, "flat");
        var menuOpen = TextButton(T("viewDetails"), () => { menu.Popdown(); item.ShowDetailCommand.Execute(); }, "flat");
        menuBox.Append(menuRead);
        menuBox.Append(menuOpen);
        menu.SetChild(menuBox);
        menu.SetParent(row);
        var rightClick = Gtk.GestureClick.New();
        rightClick.SetButton(3);
        rightClick.OnPressed += (_, args) =>
        {
            menuRead.SetSensitive(item.IsUnread);
            menu.SetPointingTo(new Gdk.Rectangle { X = (int)args.X, Y = (int)args.Y, Width = 1, Height = 1 });
            menu.Popup();
        };
        row.AddController(rightClick);

        subscriptions.Add(item.Bind(() =>
        {
            dot.SetOpacity(item.IsUnread ? 1 : 0);
            if (item.IsUnread) title.AddCssClass("ppvpn-msg-title-unread");
            else title.RemoveCssClass("ppvpn-msg-title-unread");
            time.SetText(item.RelativeTime);
        }, nameof(item.IsUnread), "RelativeTime"));
        return row;
    }

    private static Gtk.Widget DetailView(InboxViewModel vm)
    {
        var root = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        var header = Adw.HeaderBar.New();
        var back = Gtk.Button.New();
        var backContent = Adw.ButtonContent.New();
        backContent.SetIconName("go-previous-symbolic");
        backContent.SetLabel(T("msgCenter"));
        back.SetChild(backContent);
        back.OnClicked += (_, _) => vm.BackCommand.Execute();
        header.PackStart(back);
        var position = Label(null, "dim-label", "numeric");
        header.SetTitleWidget(position);
        var navigation = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        navigation.AddCssClass("linked");
        var previous = IconButton("go-up-symbolic", T("prevMsg"), () => vm.PreviousCommand.Execute());
        var next = IconButton("go-down-symbolic", T("nextMsg"), () => vm.NextCommand.Execute());
        navigation.Append(previous);
        navigation.Append(next);
        header.PackEnd(navigation);
        root.Append(header);

        var content = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        var bar = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        bar.AddCssClass("ppvpn-msg-bar");
        var body = Gtk.Box.New(Gtk.Orientation.Vertical, 10);
        body.SetMarginTop(16);
        body.SetMarginBottom(16);
        body.SetMarginStart(16);
        body.SetMarginEnd(16);
        body.SetHexpand(true);
        var kind = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        var icon = Gtk.Box.New(Gtk.Orientation.Vertical, 0);
        icon.AddCssClass("ppvpn-type-icon");
        icon.SetValign(Gtk.Align.Center);
        icon.SetSizeRequest(32, 32);
        icon.SetVexpand(false);
        var image = Gtk.Image.New();
        image.SetVexpand(true);
        image.SetValign(Gtk.Align.Center);
        icon.Append(image);
        var type = Label(null, "heading");
        var level = Label(null, "ppvpn-level-tag");
        level.SetValign(Gtk.Align.Center);
        kind.Append(icon);
        kind.Append(type);
        kind.Append(level);
        var title = Label(null, "title-3");
        title.SetWrap(true);
        var time = Label(null, "dim-label");
        var text = Label(null);
        text.SetWrap(true);
        text.SetWrapMode(Pango.WrapMode.WordChar);
        text.SetSelectable(true);
        var linkBox = Gtk.Box.New(Gtk.Orientation.Vertical, 4);
        linkBox.SetMarginTop(8);
        var link = TextButton(T("viewDetails") + " ↗", () => vm.Detail?.OpenLinkCommand.Execute(), "suggested-action", "pill");
        link.SetHalign(Gtk.Align.Start);
        linkBox.Append(link);
        linkBox.Append(Label(T("openLinkHint"), "ppvpn-weakest"));
        body.Append(kind);
        body.Append(title);
        body.Append(time);
        body.Append(text);
        body.Append(linkBox);
        content.Append(bar);
        content.Append(body);
        var scroller = Gtk.ScrolledWindow.New();
        scroller.SetPolicy(Gtk.PolicyType.Never, Gtk.PolicyType.Automatic);
        scroller.SetVexpand(true);
        scroller.SetChild(content);
        root.Append(scroller);

        var footer = Gtk.Box.New(Gtk.Orientation.Horizontal, 0);
        footer.SetMarginStart(12);
        footer.SetMarginEnd(12);
        footer.SetMarginTop(6);
        footer.SetMarginBottom(8);
        var toggleRead = TextButton("", () => vm.ToggleDetailReadCommand.Execute(), "flat");
        footer.Append(toggleRead);
        root.Append(footer);

        vm.Bind(() =>
        {
            // A clicked push without an inbox message: no place in the list, no read state.
            var readOnly = vm.IsDetailReadOnly;
            position.SetVisible(!readOnly);
            navigation.SetVisible(!readOnly);
            footer.SetVisible(!readOnly);
            position.SetText(vm.PositionText);
            previous.SetSensitive(vm.CanPrevious);
            next.SetSensitive(vm.CanNext);
            toggleRead.SetLabel(vm.DetailReadActionText);
            if (vm.Detail is not { } item) return;
            var tone = Theme.ToneClass(item.Tone);
            bar.SetToneClass(tone);
            icon.SetToneClass(tone);
            level.SetToneClass(tone);
            image.SetFromIconName(TypeIcon(item.Category));
            type.SetText(item.TypeLabel);
            level.SetVisible(item.HasSeverityLabel);
            level.SetText(item.SeverityLabel);
            title.SetText(item.Title);
            time.SetText(item.DetailTimeText);
            text.SetText(item.Content);
            linkBox.SetVisible(item.HasLink);
            scroller.GetVadjustment().SetValue(0);
        }, nameof(vm.Detail), nameof(vm.IsDetailReadOnly), nameof(vm.PositionText), nameof(vm.CanPrevious), nameof(vm.CanNext),
            nameof(vm.DetailReadActionText));
        return root;
    }
}
