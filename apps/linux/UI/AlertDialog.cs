namespace PPVPN.Linux.UI;

/// <summary>
/// The design's AdwAlertDialog (libadwaita 1.5), built from an Adw.Window for libadwaita 1.1:
/// heading, body, and a cancel / response button pair; Escape or closing cancels.
/// </summary>
public static class AlertDialog
{
    /// <returns>True when <paramref name="confirm"/> was chosen.</returns>
    public static Task<bool> ShowAsync(Gtk.Window parent, string heading, string body, string? cancel, string confirm, bool destructive)
    {
        var result = new TaskCompletionSource<bool>();
        var dialog = Adw.Window.New();
        dialog.SetTransientFor(parent);
        dialog.SetModal(true);
        dialog.SetResizable(false);
        dialog.SetDefaultSize(360, -1);
        dialog.AddCssClass("ppvpn-alert");

        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 10);
        box.SetMarginTop(24);
        box.SetMarginBottom(24);
        box.SetMarginStart(24);
        box.SetMarginEnd(24);
        var title = Gtk.Label.New(heading);
        title.AddCssClass("title-2");
        title.SetWrap(true);
        title.SetJustify(Gtk.Justification.Center);
        var text = Gtk.Label.New(body);
        text.SetWrap(true);
        text.SetJustify(Gtk.Justification.Center);
        text.SetMaxWidthChars(40);
        var buttons = Gtk.Box.New(Gtk.Orientation.Horizontal, 12);
        buttons.SetHomogeneous(true);
        buttons.SetMarginTop(14);

        void Finish(bool value)
        {
            if (result.TrySetResult(value)) dialog.Close();
        }

        if (cancel is not null)
        {
            var cancelButton = Gtk.Button.NewWithLabel(cancel);
            cancelButton.OnClicked += (_, _) => Finish(false);
            buttons.Append(cancelButton);
        }
        var ok = Gtk.Button.NewWithLabel(confirm);
        ok.AddCssClass(destructive ? "destructive-action" : "suggested-action");
        ok.OnClicked += (_, _) => Finish(true);
        buttons.Append(ok);

        box.Append(title);
        box.Append(text);
        box.Append(buttons);
        dialog.SetContent(box);

        var keys = Gtk.EventControllerKey.New();
        keys.OnKeyPressed += (_, args) =>
        {
            if (args.Keyval != Gdk.Constants.KEY_Escape) return false;
            Finish(false);
            return true;
        };
        dialog.AddController(keys);
        dialog.OnCloseRequest += (_, _) =>
        {
            result.TrySetResult(false);
            return false;
        };
        dialog.SetDefaultWidget(ok);
        dialog.Present();
        ok.GrabFocus();
        return result.Task;
    }
}
