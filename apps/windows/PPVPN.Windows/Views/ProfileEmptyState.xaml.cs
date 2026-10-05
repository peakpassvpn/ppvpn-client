using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using PPVPN.App.Core.ViewModels;
using PPVPN.Windows.Strings;

namespace PPVPN.Windows.Views;

public sealed partial class ProfileEmptyState : UserControl
{
    public ProfileEmptyState()
    {
        InitializeComponent();
    }

    public MainViewModel ViewModel => App.ViewModel;

    /// <summary>"Switch team ▾" opens the account menu anchored to the button.</summary>
    void OnSwitchTeam(object sender, RoutedEventArgs e) => App.Current.MainWindow?.OpenAccountMenu(SwitchTeamButton);
}

public static class ProfileEmptyStateText
{
    /// <summary>"刷新", "…" while refreshing.</summary>
    public static string Refresh(bool running) => running ? "…" : Loc.Get("refresh");

    public static string Glyph(AccessState access) => access switch
    {
        AccessState.NoSubscription => "\uE7BF",  // shopping cart
        AccessState.Expired => "\uE823",         // clock
        AccessState.TeamDisabled => "\uE716",    // people
        _ => "\uE946",
    };
}
