using System.Windows.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using PPVPN.Windows.Strings;

namespace PPVPN.Windows.Controls;

/// <summary>
/// A copy button: copy icon + <see cref="Label"/>; after a click it shows "✓ 已复制" in the success
/// colour for 1.6 s (design: "复制成功为按钮原地 ✓ 1.6s").
/// </summary>
public sealed partial class CopyButton : Button
{
    public static readonly DependencyProperty LabelProperty =
        DependencyProperty.Register(nameof(Label), typeof(string), typeof(CopyButton), new PropertyMetadata("", (d, _) => ((CopyButton)d).ShowLabel()));

    public static readonly DependencyProperty CopyCommandProperty =
        DependencyProperty.Register(nameof(CopyCommand), typeof(ICommand), typeof(CopyButton), new PropertyMetadata(null));

    public static readonly DependencyProperty IconOnlyProperty =
        DependencyProperty.Register(nameof(IconOnly), typeof(bool), typeof(CopyButton), new PropertyMetadata(false, (d, _) => ((CopyButton)d).ShowLabel()));

    readonly FontIcon _icon = new() { FontSize = 14, Glyph = "" };
    readonly TextBlock _text = new() { VerticalAlignment = VerticalAlignment.Center };
    int _generation;

    public CopyButton()
    {
        DefaultStyleKey = typeof(Button);
        MinHeight = 32;
        Content = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Children = { _icon, _text } };
        ShowLabel();
        Click += OnClick;
    }

    public string Label
    {
        get => (string)GetValue(LabelProperty);
        set => SetValue(LabelProperty, value);
    }

    /// <summary>Only the icon (copy → ✓); the text goes to the tooltip.</summary>
    public bool IconOnly
    {
        get => (bool)GetValue(IconOnlyProperty);
        set => SetValue(IconOnlyProperty, value);
    }

    public ICommand? CopyCommand
    {
        get => (ICommand?)GetValue(CopyCommandProperty);
        set => SetValue(CopyCommandProperty, value);
    }

    async void OnClick(object sender, RoutedEventArgs e)
    {
        if (CopyCommand is not { } command || !command.CanExecute(null)) return;
        command.Execute(null);
        var generation = ++_generation;
        var success = Ui.ToneBrush(PPVPN.App.Core.ViewModels.ConnectionTone.Ok);
        _icon.Glyph = "";
        _icon.Foreground = success;
        _text.Text = Loc.Get("copied");
        _text.Foreground = success;
        await Task.Delay(1600);
        if (generation == _generation) ShowLabel();
    }

    void ShowLabel()
    {
        _icon.Glyph = "";
        _icon.ClearValue(FontIcon.ForegroundProperty);
        _text.ClearValue(TextBlock.ForegroundProperty);
        _text.Text = string.IsNullOrEmpty(Label) ? Loc.Get("copy") : Label;
        _text.Visibility = IconOnly ? Visibility.Collapsed : Visibility.Visible;
    }
}
