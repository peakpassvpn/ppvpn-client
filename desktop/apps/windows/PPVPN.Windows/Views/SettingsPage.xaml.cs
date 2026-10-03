using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.Windows.Views;

/// <summary>
/// Settings: General (appearance, launch at login, update checks), Account (signed in only),
/// Advanced (connection method, routing mode, system service, API override), About. Reachable signed out.
/// </summary>
public sealed partial class SettingsPage : Page
{
    bool _loading;

    public SettingsPage()
    {
        InitializeComponent();
        ActualThemeChanged += (_, _) => Bindings.Update();
        ViewModel.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName == nameof(SettingsViewModel.SelectedConnectionMode)) SyncMode();
            if (e.PropertyName == nameof(SettingsViewModel.SelectedRoutingMode)) SyncRouting();
        };
    }

    public SettingsViewModel ViewModel => App.ViewModel.Settings;

    public MainViewModel Main => App.ViewModel;

    public static Brush ServiceDot(bool installed) => Ui.ToneBrush(installed ? ConnectionTone.Ok : ConnectionTone.Idle);

    /// <summary>The expiry date in the danger colour once expired.</summary>
    public static Brush ExpiresBrush(bool expired) =>
        expired ? Ui.ToneBrush(ConnectionTone.Error) : Ui.ToneBrush(ConnectionTone.Idle);

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        ViewModel.Refresh();
        SyncMode();
        SyncRouting();
    }

    /// <summary>Connection method radios: titles and descriptions from <see cref="SettingsViewModel.ConnectionModes"/>.</summary>
    void SyncMode()
    {
        _loading = true;
        foreach (var option in ViewModel.ConnectionModes)
        {
            var (title, description) = option.Mode == ConnectionMode.Compatible
                ? (ModeCompatTitle, ModeCompatDescription)
                : (ModeEnhancedTitle, ModeEnhancedDescription);
            title.Text = option.Title;
            description.Text = option.Description;
        }
        var compatible = ViewModel.SelectedConnectionMode?.Mode == ConnectionMode.Compatible;
        (compatible ? ModeCompat : ModeEnhanced).IsChecked = true;
        _loading = false;
    }

    void OnModeChecked(object sender, RoutedEventArgs e)
    {
        if (_loading || sender is not FrameworkElement { Tag: string tag }) return;
        var mode = tag == "Compatible" ? ConnectionMode.Compatible : ConnectionMode.Enhanced;
        if (ViewModel.ConnectionModes.FirstOrDefault(o => o.Mode == mode) is { } option)
            ViewModel.SetConnectionModeCommand.Execute(option);
    }

    /// <summary>Routing mode radios: titles and descriptions from <see cref="SettingsViewModel.RoutingModes"/>.</summary>
    void SyncRouting()
    {
        _loading = true;
        foreach (var option in ViewModel.RoutingModes)
        {
            var (title, description) = option.Mode == RoutingMode.Global
                ? (RoutingGlobalTitle, RoutingGlobalDescription)
                : (RoutingRulesTitle, RoutingRulesDescription);
            title.Text = option.Title;
            description.Text = option.Description;
        }
        var global = ViewModel.SelectedRoutingMode?.Mode == RoutingMode.Global;
        (global ? RoutingGlobal : RoutingRules).IsChecked = true;
        _loading = false;
    }

    void OnRoutingChecked(object sender, RoutedEventArgs e)
    {
        if (_loading || sender is not FrameworkElement { Tag: string tag }) return;
        var mode = tag == "Global" ? RoutingMode.Global : RoutingMode.Rules;
        if (ViewModel.RoutingModes.FirstOrDefault(o => o.Mode == mode) is { } option)
            ViewModel.SetRoutingModeCommand.Execute(option);
    }

    void OnBack(object sender, RoutedEventArgs e) => App.Current.MainWindow?.NavigateTo("overview");
}
