using System.ComponentModel;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Animation;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.Windows.Views;

public sealed partial class OverviewPage : Page
{
    Storyboard? _spin;

    public OverviewPage()
    {
        InitializeComponent();
        // Tone colours are resolved in code; refresh them with the theme.
        ActualThemeChanged += (_, _) => Bindings.Update();
        ViewModel.PropertyChanged += OnViewModelChanged;
        UpdateSpin();
    }

    public MainViewModel ViewModel => App.ViewModel;

    /// <summary>Status circle icon (26, filled): shield / network when connected, sync while busy, error / warning.</summary>
    public static string Glyph(ConnectState state, ConnectionMode mode) => state switch
    {
        ConnectState.On => mode == ConnectionMode.Compatible ? "\uE968" : "\uEA18",
        ConnectState.Failed => "\uE783",
        ConnectState.Occupied => "\uE7BA",
        ConnectState.Off => "\uE7E8",
        _ => "\uE895",
    };

    /// <summary>"· detail" after the node name; nothing when there is no detail.</summary>
    public static string Detail(string detail) => string.IsNullOrEmpty(detail) ? "" : "· " + detail;


    public static string NoticeGlyph(ConnectionTone tone) => tone == ConnectionTone.Warn ? "\uE7BA" : "\uEA39";

    /// <summary>Combo items show a measured latency only.</summary>
    public static string ComboLatency(LatencyKind kind, string text) => kind == LatencyKind.Value ? text : "";

    /// <summary>
    /// Writes the picked scope back only when it changed (like the line picker on Nodes); a two-way
    /// binding crashed with a stack overflow on switching scopes.
    /// </summary>
    void OnProxyScopeChanged(object sender, SelectionChangedEventArgs e)
    {
        if (e.AddedItems.FirstOrDefault() is LocalProxyScopeOption scope && scope != ViewModel.SelectedProxyScope)
            ViewModel.SelectedProxyScope = scope;
    }

    void OnViewModelChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(MainViewModel.IsConnectionBusy) || e.PropertyName == nameof(MainViewModel.ConnectionTone)) UpdateSpin();
    }

    /// <summary>The status icon spins while busy (1 s linear).</summary>
    void UpdateSpin()
    {
        if (ViewModel.IsConnectionBusy && _spin is null)
        {
            var animation = new DoubleAnimation
            {
                From = 0,
                To = 360,
                Duration = new Microsoft.UI.Xaml.Duration(TimeSpan.FromSeconds(1)),
                RepeatBehavior = RepeatBehavior.Forever,
            };
            Storyboard.SetTarget(animation, StatusSpin);
            Storyboard.SetTargetProperty(animation, "Angle");
            _spin = new Storyboard { Children = { animation } };
            _spin.Begin();
        }
        else if (!ViewModel.IsConnectionBusy && _spin is not null)
        {
            _spin.Stop();
            _spin = null;
            StatusSpin.Angle = 0;
        }
    }
}
