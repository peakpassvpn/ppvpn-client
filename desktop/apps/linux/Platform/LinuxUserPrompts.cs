using PPVPN.App.Core.ViewModels;
using PPVPN.Linux.UI;

namespace PPVPN.Linux.Platform;

/// <summary>
/// The view models' modal questions and one-time errors, as AdwAlertDialog-style windows (the
/// real AdwAlertDialog is libadwaita 1.5; Ubuntu 22.04 has 1.1). One at a time, over the main
/// window, which is shown first when hidden.
/// </summary>
public sealed class LinuxUserPrompts(ILocalizer strings, Func<Gtk.Window> mainWindow) : IUserPrompts
{
    private readonly SemaphoreSlim _one = new(1, 1);

    public async Task<bool> ConfirmAsync(PromptKind kind)
    {
        var texts = PromptTexts.For(kind, strings);
        await _one.WaitAsync();
        try
        {
            var destructive = kind is PromptKind.UninstallService or PromptKind.SignOut;
            return await AlertDialog.ShowAsync(Parent(), texts.Title, texts.Message, texts.Cancel, texts.Confirm, destructive);
        }
        finally
        {
            _one.Release();
        }
    }

    public async Task ShowErrorAsync(string title, string message)
    {
        await _one.WaitAsync();
        try
        {
            await AlertDialog.ShowAsync(Parent(), title, message, null, strings.Get("ok"), destructive: false);
        }
        finally
        {
            _one.Release();
        }
    }

    private Gtk.Window Parent()
    {
        var window = mainWindow();
        if (!window.IsVisible()) window.Present();
        return window;
    }
}
