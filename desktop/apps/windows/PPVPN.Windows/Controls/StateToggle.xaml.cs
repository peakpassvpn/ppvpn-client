using System.Windows.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media.Animation;
using PPVPN.App.Core.ViewModels;
using Windows.System;

namespace PPVPN.Windows.Controls;

/// <summary>
/// The connect switch: off / on / busy (indeterminate). A click runs <see cref="Command"/>; the
/// state always comes from the view model, never from the click itself.
/// </summary>
public sealed partial class StateToggle : UserControl
{
    const double Travel = 20; // 40 wide, knob 16 with a 2px inset
    static readonly Duration KnobDuration = new(TimeSpan.FromMilliseconds(150));

    public static readonly DependencyProperty StateProperty =
        DependencyProperty.Register(nameof(State), typeof(SwitchVisual), typeof(StateToggle), new PropertyMetadata(SwitchVisual.Off, OnVisualChanged));

    public static readonly DependencyProperty IsInteractiveProperty =
        DependencyProperty.Register(nameof(IsInteractive), typeof(bool), typeof(StateToggle), new PropertyMetadata(true, OnVisualChanged));

    public static readonly DependencyProperty CommandProperty =
        DependencyProperty.Register(nameof(Command), typeof(ICommand), typeof(StateToggle), new PropertyMetadata(null));

    bool _hover;

    public StateToggle()
    {
        InitializeComponent();
        Loaded += (_, _) => Apply(animate: false);
        PointerEntered += (_, _) => { _hover = true; Apply(animate: true); };
        PointerExited += (_, _) => { _hover = false; Apply(animate: true); };
        Tapped += (_, e) => { e.Handled = true; Invoke(); };
        KeyDown += OnKeyDown;
    }

    public SwitchVisual State
    {
        get => (SwitchVisual)GetValue(StateProperty);
        set => SetValue(StateProperty, value);
    }

    public bool IsInteractive
    {
        get => (bool)GetValue(IsInteractiveProperty);
        set => SetValue(IsInteractiveProperty, value);
    }

    public ICommand? Command
    {
        get => (ICommand?)GetValue(CommandProperty);
        set => SetValue(CommandProperty, value);
    }

    void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key is VirtualKey.Space or VirtualKey.Enter)
        {
            e.Handled = true;
            Invoke();
        }
    }

    void Invoke()
    {
        if (!IsInteractive || Command is not { } command || !command.CanExecute(null)) return;
        command.Execute(null);
    }

    static void OnVisualChanged(DependencyObject sender, DependencyPropertyChangedEventArgs args) =>
        ((StateToggle)sender).Apply(animate: true);

    void Apply(bool animate)
    {
        var state = State;
        VisualStateManager.GoToState(this, state == SwitchVisual.Indeterminate ? "Busy" : state.ToString(), false);
        AutomationProperties.SetItemStatus(this, state.ToString());

        // Not interactive: dim, except while busy (the spinner stays at full opacity).
        var dim = !IsInteractive && state != SwitchVisual.Indeterminate;
        Track.Opacity = dim ? 0.45 : 1;
        KnobFill.Opacity = dim ? 0.45 : 1;

        var x = 2 + state switch { SwitchVisual.On => Travel, SwitchVisual.Indeterminate => Travel / 2, _ => 0 };
        var scale = state == SwitchVisual.Indeterminate ? 1.0 : _hover && IsInteractive ? 0.875 : 0.75;
        if (!animate || !IsLoaded)
        {
            KnobTransform.TranslateX = x;
            KnobTransform.ScaleX = KnobTransform.ScaleY = scale;
            return;
        }
        var storyboard = new Storyboard();
        Add(storyboard, "TranslateX", x);
        Add(storyboard, "ScaleX", scale);
        Add(storyboard, "ScaleY", scale);
        storyboard.Begin();
    }

    void Add(Storyboard storyboard, string property, double to)
    {
        var animation = new DoubleAnimation
        {
            To = to,
            Duration = KnobDuration,
            EasingFunction = new CubicEase { EasingMode = EasingMode.EaseOut },
        };
        Storyboard.SetTarget(animation, KnobTransform);
        Storyboard.SetTargetProperty(animation, property);
        storyboard.Children.Add(animation);
    }
}
