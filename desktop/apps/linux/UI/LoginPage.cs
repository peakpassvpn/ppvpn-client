using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>Browser device sign-in: signed out, waiting for the code, or an error.</summary>
public static class LoginPage
{
    public static Gtk.Widget Create(MainViewModel vm)
    {
        var box = Gtk.Box.New(Gtk.Orientation.Vertical, 12);
        box.SetHalign(Gtk.Align.Center);
        box.SetValign(Gtk.Align.Center);
        box.SetMarginStart(24);
        box.SetMarginEnd(24);

        var icon = Gtk.Image.NewFromIconName("com.peakpassvpn.ppvpn.desktop");
        icon.SetPixelSize(76);
        var errorIcon = Gtk.Image.NewFromIconName("dialog-error-symbolic");
        errorIcon.SetPixelSize(64);
        errorIcon.AddCssClass("tone-error");
        var title = Gtk.Label.New(null);
        title.AddCssClass("title-1");
        title.SetWrap(true);
        title.SetJustify(Gtk.Justification.Center);
        var subtitle = Gtk.Label.New(null);
        subtitle.SetWrap(true);
        subtitle.SetJustify(Gtk.Justification.Center);
        subtitle.SetMaxWidthChars(46);
        subtitle.AddCssClass("dim-label");

        // Signed out / error: one pill button.
        var primary = TextButton("", () => vm.SignInCommand.Execute(), "suggested-action", "pill");
        primary.SetHalign(Gtk.Align.Center);
        primary.SetMarginTop(10);

        // Waiting: the code, copy, countdown, cancel / reopen.
        var waiting = Gtk.Box.New(Gtk.Orientation.Vertical, 10);
        var codeRow = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        codeRow.SetHalign(Gtk.Align.Center);
        var code = Gtk.Label.New(null);
        code.AddCssClass("title-1");
        code.AddCssClass("ppvpn-mono");
        code.SetSelectable(true);
        codeRow.Append(code);
        codeRow.Append(CopyButton(() => vm.CopyCodeCommand, ""));
        var countdownRow = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        countdownRow.SetHalign(Gtk.Align.Center);
        var spinner = Gtk.Spinner.New();
        spinner.Start();
        var countdown = Label(null, "dim-label", "numeric");
        countdownRow.Append(spinner);
        countdownRow.Append(countdown);
        var buttons = Gtk.Box.New(Gtk.Orientation.Horizontal, 12);
        buttons.SetHalign(Gtk.Align.Center);
        buttons.SetMarginTop(6);
        buttons.Append(TextButton(T("cancel"), () => vm.CancelSignInCommand.Execute()));
        var reopen = TextButton(T("reopen"), () => vm.ReopenBrowserCommand.Execute());
        buttons.Append(reopen);
        // The browser could not be opened: say so and offer the link instead.
        var linkBox = Gtk.Box.New(Gtk.Orientation.Vertical, 4);
        var failRow = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        failRow.SetHalign(Gtk.Align.Center);
        failRow.AddCssClass("tone-warn");
        failRow.Append(Gtk.Image.NewFromIconName("dialog-warning-symbolic"));
        failRow.Append(Label(T("browserFailT"), "heading"));
        var linkRow = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        linkRow.SetHalign(Gtk.Align.Center);
        var link = Label(null, "ppvpn-mono", "dim-label");
        link.SetSelectable(true);
        link.SetEllipsize(Pango.EllipsizeMode.Middle);
        link.SetMaxWidthChars(44);
        linkRow.Append(link);
        linkRow.Append(CopyButton(() => vm.CopyVerificationUrlCommand, ""));
        waiting.Append(codeRow);
        waiting.Append(countdownRow);
        linkBox.Append(failRow);
        linkBox.Append(linkRow);
        waiting.Append(linkBox);
        waiting.Append(buttons);

        box.Append(icon);
        box.Append(errorIcon);
        box.Append(title);
        box.Append(subtitle);
        box.Append(primary);
        box.Append(waiting);

        vm.Bind(() =>
        {
            var awaiting = vm.IsAwaiting;
            var error = vm.HasLoginError && !awaiting;
            icon.SetVisible(!error);
            errorIcon.SetVisible(error);
            title.SetText(awaiting ? T("waitTitle") : error ? vm.LoginErrorTitle : T("loginTitle"));
            // The browser did not open: no "your browser opened…"; the link box says so and
            // offers the verification URL, and the button opens the browser (not "reopen").
            var waitText = vm.BrowserNotOpened ? T("browserFailD") : T("waitSub");
            subtitle.SetText(awaiting ? waitText : error ? vm.LoginErrorMessage : T("loginSub"));
            primary.SetVisible(!awaiting);
            primary.SetLabel(error ? vm.LoginRetryText : T("loginBtn"));
            primary.SetSensitive(!vm.IsStartingLogin);
            waiting.SetVisible(awaiting);
            code.SetText(vm.UserCode);
            countdown.SetText(vm.CountdownText);
            linkBox.SetVisible(vm.BrowserNotOpened);
            reopen.SetLabel(T(vm.BrowserNotOpened ? "openBrowser" : "reopen"));
            link.SetText(vm.VerificationUrl);
        }, nameof(vm.Stage), nameof(vm.IsStartingLogin), nameof(vm.LoginErrorTitle), nameof(vm.LoginErrorMessage),
            nameof(vm.LoginRetryText), nameof(vm.UserCode), nameof(vm.CountdownText), nameof(vm.BrowserNotOpened),
            nameof(vm.VerificationUrl));

        return box;
    }
}
