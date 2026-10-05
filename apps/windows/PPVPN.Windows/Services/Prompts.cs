using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using PPVPN.App.Core.ViewModels;
using PPVPN.Windows.Strings;

namespace PPVPN.Windows.Services;

/// <summary>
/// One-off results and confirmations as ContentDialogs (design: "一次性结果用模态"). WinUI allows
/// one open ContentDialog per XamlRoot, so dialogs are queued per root. When the target window is
/// hidden (the action came from the tray), it is shown first.
/// This is App.Core's <see cref="IUserPrompts"/> (texts: <see cref="PromptTexts"/>).
/// </summary>
public sealed class Prompts : IUserPrompts
{
    readonly Func<XamlRoot?> _root;
    readonly Action _showWindow;
    readonly Dictionary<XamlRoot, Task> _queues = [];

    /// <param name="root">The XamlRoot to show on (the main window's), or null while it has none.</param>
    /// <param name="showWindow">Brings the main window up (it may be hidden in the tray).</param>
    public Prompts(Func<XamlRoot?> root, Action showWindow)
    {
        _root = root;
        _showWindow = showWindow;
    }

    public Task<bool> ConfirmAsync(PromptKind kind)
    {
        var texts = PromptTexts.For(kind, Loc.Strings);
        return ConfirmAsync(texts.Title, texts.Message, texts.Confirm, texts.Cancel);
    }

    public Task ShowErrorAsync(string title, string message) => ShowErrorAsync(title, message, null);

    /// <summary>Title, message, a primary (accent) button and Cancel. True when confirmed.</summary>
    public async Task<bool> ConfirmAsync(string title, string message, string primary, string cancel, XamlRoot? root = null)
    {
        var result = await ShowAsync(new ContentDialog
        {
            Title = title,
            Content = Body(message),
            PrimaryButtonText = primary,
            CloseButtonText = cancel,
            DefaultButton = ContentDialogButton.Primary,
        }, root);
        return result == ContentDialogResult.Primary;
    }

    /// <summary>A one-off failure: title, message and OK.</summary>
    public async Task ShowErrorAsync(string title, string message, XamlRoot? root) =>
        await ShowAsync(new ContentDialog
        {
            Title = title,
            Content = Body(message),
            CloseButtonText = Loc.Get("ok"),
            DefaultButton = ContentDialogButton.Close,
        }, root);

    async Task<ContentDialogResult> ShowAsync(ContentDialog dialog, XamlRoot? root)
    {
        if (root is null)
        {
            _showWindow();
            root = _root();
        }
        if (root is null) return ContentDialogResult.None;
        dialog.XamlRoot = root;
        dialog.Style = (Style)Application.Current.Resources["DefaultContentDialogStyle"];
        if (root.Content is FrameworkElement content) dialog.RequestedTheme = content.ActualTheme;

        var previous = _queues.TryGetValue(root, out var queued) ? queued : Task.CompletedTask;
        var gate = new TaskCompletionSource<ContentDialogResult>();
        _queues[root] = gate.Task;
        try
        {
            await previous;
        }
        catch (Exception) { }
        try
        {
            gate.SetResult(await dialog.ShowAsync());
        }
        catch (Exception error)
        {
            gate.SetResult(ContentDialogResult.None);
            App.Log?.Warn($"dialog failed: {error.Message}");
        }
        if (_queues.TryGetValue(root, out var last) && last == gate.Task) _queues.Remove(root);
        return await gate.Task;
    }

    static TextBlock Body(string message) => new() { Text = message, TextWrapping = TextWrapping.Wrap, MaxWidth = 392 };
}
