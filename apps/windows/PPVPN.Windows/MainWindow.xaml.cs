using System.ComponentModel;
using System.Globalization;
using System.Runtime.InteropServices;
using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media.Animation;
using PPVPN.App.Core.ViewModels;
using PPVPN.Windows.Platform;
using PPVPN.Windows.Services;
using PPVPN.Windows.Strings;
using PPVPN.Windows.Views;
using Windows.Graphics;
using Windows.System;
using Windows.UI;

namespace PPVPN.Windows;

public sealed partial class MainWindow : Window
{
    static readonly Dictionary<string, Type> Pages = new()
    {
        ["overview"] = typeof(OverviewPage),
        ["nodes"] = typeof(NodesPage),
        ["logs"] = typeof(LogsPage),
        ["settings"] = typeof(SettingsPage),
    };

    readonly JsonSettingsStore _settings;
    bool _closeForReal;
    string _currentPage = "";

    public MainWindow(MainViewModel viewModel, JsonSettingsStore settings)
    {
        ViewModel = viewModel;
        _settings = settings;
        InitializeComponent();
        Ui.Root = Root;

        Title = "PPVPN";
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Standard;
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico"));
        if (AppWindow.Presenter is OverlappedPresenter presenter)
        {
            presenter.PreferredMinimumWidth = 720;
            presenter.PreferredMinimumHeight = 500;
        }
        if (!WindowPlacement.Restore(AppWindow, settings.GetWindowPlacement("main"))) WindowPlacement.SizeAndCenter(AppWindow, 960, 640);

        // Closing hides to the tray; Quit (tray menu) really closes. The first close says so once.
        AppWindow.Closing += (sender, args) =>
        {
            SavePlacement();
            if (_closeForReal) return;
            args.Cancel = true;
            sender.Hide();
            if (!_settings.BackgroundHintShown)
            {
                _settings.BackgroundHintShown = true;
                _settings.Save();
                try { AppNotifications.ShowNotice(Loc.Get("stillRunning"), Loc.Get("stillRunningD")); }
                catch (Exception error) { App.Log?.Warn($"first-close notice failed: {error.Message}"); }
            }
        };

        Root.ActualThemeChanged += (_, _) => { UpdateCaptionButtons(); Bindings.Update(); };
        UpdateCaptionButtons();
        AddAccelerators();

        ViewModel.PropertyChanged += OnViewModelChanged;
        NavigateTo("overview");
        UpdateContent();
    }

    public MainViewModel ViewModel { get; }

    public string CurrentPage => _currentPage;

    public void ShowAndFocus()
    {
        AppWindow.Show();
        if (AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter) presenter.Restore();
        Activate();
        NativeMethods.SetForegroundWindow(WinRT.Interop.WindowNative.GetWindowHandle(this));
    }

    public bool IsShown => AppWindow.IsVisible;

    public void CloseForReal()
    {
        _closeForReal = true;
        Close();
    }

    public void SavePlacement()
    {
        _settings.SetWindowPlacement("main", WindowPlacement.Capture(AppWindow));
        _settings.Save();
    }

    public void ApplyAppearance(Appearance appearance)
    {
        Root.RequestedTheme = appearance switch
        {
            Appearance.Light => ElementTheme.Light,
            Appearance.Dark => ElementTheme.Dark,
            _ => ElementTheme.Default,
        };
        UpdateCaptionButtons();
    }

    /// <summary>Show a page: overview, nodes, logs, settings.</summary>
    public void NavigateTo(string page)
    {
        if (!Pages.ContainsKey(page)) page = "overview";
        if (page == "settings")
        {
            Nav.SelectedItem = null;
            Show("settings");
        }
        else if (Nav.MenuItems.OfType<NavigationViewItem>().FirstOrDefault(i => (string)i.Tag == page) is { } item)
        {
            if (ReferenceEquals(Nav.SelectedItem, item)) Show(page);
            else Nav.SelectedItem = item;
        }
    }

    /// <summary>Opens the account menu (the "switch team" button and <c>--demo-teams</c>).</summary>
    public void OpenAccountMenu(FrameworkElement? anchor = null)
    {
        if (anchor is null)
        {
            AccountButton.Flyout.ShowAt(AccountButton);
            return;
        }
        AccountMenu.ShowAt(anchor, new Microsoft.UI.Xaml.Controls.Primitives.FlyoutShowOptions
        {
            Placement = Microsoft.UI.Xaml.Controls.Primitives.FlyoutPlacementMode.BottomEdgeAlignedLeft,
        });
    }

    void OnNavigationSelectionChanged(NavigationView sender, NavigationViewSelectionChangedEventArgs args)
    {
        if (args.SelectedItemContainer?.Tag is string tag) Show(tag);
    }

    void Show(string tag)
    {
        SettingsPill.Visibility = tag == "settings" ? Visibility.Visible : Visibility.Collapsed;
        if (tag != _currentPage && Pages.TryGetValue(tag, out var page))
        {
            _currentPage = tag;
            ContentFrame.Navigate(page, null, new EntranceNavigationTransitionInfo());
        }
        UpdateContent();
    }

    /// <summary>Signed out: the login view, unless Settings is open (reachable signed out).</summary>
    void UpdateContent()
    {
        var login = ViewModel.ShowLogin && _currentPage != "settings";
        Login.Visibility = login ? Visibility.Visible : Visibility.Collapsed;
        ContentFrame.Visibility = login ? Visibility.Collapsed : Visibility.Visible;
    }

    void OnViewModelChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName != nameof(MainViewModel.Stage)) return;
        // Back to the overview after signing in or out.
        if (ViewModel.Stage is AuthStage.SignedOut or AuthStage.SignedIn && _currentPage != "settings") NavigateTo("overview");
        UpdateContent();
    }

    void OnBellClick(object sender, RoutedEventArgs e) => App.Current.ShowMessageCenter();

    void OnSettingsClick(object sender, RoutedEventArgs e) =>
        NavigateTo(_currentPage == "settings" && !ViewModel.IsSignedIn ? "overview" : "settings");

    /// <summary>Account menu: header, teams (disabled ones greyed with a tag), account settings, sign out.</summary>
    void OnAccountMenuOpening(object? sender, object e)
    {
        var vm = ViewModel;
        AccountMenu.Items.Clear();
        AccountMenu.Items.Add(new MenuFlyoutItem
        {
            Text = vm.AccountEmail,
            Tag = vm.AvatarInitial,
            KeyboardAcceleratorTextOverride = vm.AccountSubtitle,
            Style = (Style)Root.Resources["AccountHeaderItemStyle"],
        });
        if (vm.Teams.Count > 0)
        {
            AccountMenu.Items.Add(new MenuFlyoutSeparator());
            AccountMenu.Items.Add(new MenuFlyoutItem { Text = Loc.Get("teamsHdr"), IsEnabled = false });
            foreach (var team in vm.Teams)
            {
                AccountMenu.Items.Add(new RadioMenuFlyoutItem
                {
                    Text = team.HasDisabledTag ? $"{team.Title}  ·  {team.DisabledTag}" : team.Title,
                    GroupName = "teams",
                    IsChecked = team.IsCurrent,
                    IsEnabled = team.IsSelectable,
                    Command = vm.SwitchTeamCommand,
                    CommandParameter = team,
                });
            }
        }
        AccountMenu.Items.Add(new MenuFlyoutSeparator());
        AccountMenu.Items.Add(new MenuFlyoutItem
        {
            Text = Loc.Get("accountSettings"),
            Icon = new FontIcon { Glyph = "\uE713" },
            Command = vm.OpenAccountSettingsCommand,
        });
        AccountMenu.Items.Add(new MenuFlyoutItem
        {
            Text = Loc.Get("signOut") + "…",
            Icon = new FontIcon { Glyph = "\uF3B1" },
            Command = vm.SignOutCommand,
        });
    }

    // --- keyboard -------------------------------------------------------------

    void AddAccelerators()
    {
        void Add(VirtualKey key, VirtualKeyModifiers modifiers, Action action)
        {
            var accelerator = new KeyboardAccelerator { Key = key, Modifiers = modifiers };
            accelerator.Invoked += (_, args) =>
            {
                args.Handled = true;
                action();
            };
            Root.KeyboardAccelerators.Add(accelerator);
        }
        Root.KeyboardAcceleratorPlacementMode = KeyboardAcceleratorPlacementMode.Hidden;
        Add(VirtualKey.Number1, VirtualKeyModifiers.Control, () => { if (ViewModel.IsSignedIn) NavigateTo("overview"); });
        Add(VirtualKey.Number2, VirtualKeyModifiers.Control, () => { if (ViewModel.IsSignedIn) NavigateTo("nodes"); });
        Add(VirtualKey.Number3, VirtualKeyModifiers.Control, () => { if (ViewModel.IsSignedIn) NavigateTo("logs"); });
        Add((VirtualKey)0xBC, VirtualKeyModifiers.Control, () => NavigateTo("settings")); // VK_OEM_COMMA
        Add(VirtualKey.F5, VirtualKeyModifiers.None, Refresh);
    }

    /// <summary>F5: page-dependent refresh.</summary>
    void Refresh()
    {
        if (!ViewModel.IsSignedIn) return;
        switch (_currentPage)
        {
            case "overview" or "nodes":
                ViewModel.RefreshProfileCommand.Execute(null);
                break;
            case "logs":
                ViewModel.Logs.ReloadCommand.Execute(null);
                break;
        }
    }

    /// <summary>The system caption buttons do not follow a per-window theme on their own.</summary>
    void UpdateCaptionButtons()
    {
        var dark = Root.ActualTheme == ElementTheme.Dark;
        var bar = AppWindow.TitleBar;
        bar.ButtonBackgroundColor = Colors.Transparent;
        bar.ButtonInactiveBackgroundColor = Colors.Transparent;
        bar.ButtonForegroundColor = dark ? Colors.White : Colors.Black;
        bar.ButtonInactiveForegroundColor = dark ? Color.FromArgb(0x87, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x72, 0, 0, 0);
        bar.ButtonHoverBackgroundColor = dark ? Color.FromArgb(0x15, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x09, 0, 0, 0);
        bar.ButtonHoverForegroundColor = bar.ButtonForegroundColor;
        bar.ButtonPressedBackgroundColor = dark ? Color.FromArgb(0x0B, 0xFF, 0xFF, 0xFF) : Color.FromArgb(0x06, 0, 0, 0);
        bar.ButtonPressedForegroundColor = bar.ButtonForegroundColor;
    }
}

/// <summary>Window size and position persisted as "x,y,width,height[,max]" in physical pixels.</summary>
static class WindowPlacement
{
    public static string Capture(AppWindow window)
    {
        var maximized = window.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Maximized };
        var p = window.Position;
        var s = window.Size;
        return string.Create(CultureInfo.InvariantCulture, $"{p.X},{p.Y},{s.Width},{s.Height}{(maximized ? ",max" : "")}");
    }

    /// <returns>False when there is nothing (usable) to restore.</returns>
    public static bool Restore(AppWindow window, string? value)
    {
        if (value?.Split(',') is not { Length: >= 4 } parts) return false;
        if (!int.TryParse(parts[0], CultureInfo.InvariantCulture, out var x) || !int.TryParse(parts[1], CultureInfo.InvariantCulture, out var y)
            || !int.TryParse(parts[2], CultureInfo.InvariantCulture, out var w) || !int.TryParse(parts[3], CultureInfo.InvariantCulture, out var h)
            || w < 200 || h < 200)
            return false;
        // Only when the title bar lands on a display (it may have been disconnected since).
        var area = DisplayArea.GetFromRect(new RectInt32(x, y, w, 40), DisplayAreaFallback.None);
        if (area is null) return false;
        window.MoveAndResize(new RectInt32(x, y, w, h));
        if (parts.Length > 4 && parts[4] == "max" && window.Presenter is OverlappedPresenter presenter) presenter.Maximize();
        return true;
    }

    public static void SizeAndCenter(AppWindow window, int width, int height, RectInt32? nextTo = null)
    {
        var scale = GetDpiForWindow(Win32Interop.GetWindowFromWindowId(window.Id)) / 96.0;
        var size = new SizeInt32((int)(width * scale), (int)(height * scale));
        // Beside an anchor, use the anchor's display (the new window may not be on it yet).
        var area = (nextTo is { } a
            ? DisplayArea.GetFromRect(a, DisplayAreaFallback.Nearest)
            : DisplayArea.GetFromWindowId(window.Id, DisplayAreaFallback.Primary)).WorkArea;
        size.Width = Math.Min(size.Width, area.Width);
        size.Height = Math.Min(size.Height, area.Height);
        var x = area.X + (area.Width - size.Width) / 2;
        var y = area.Y + (area.Height - size.Height) / 2;
        if (nextTo is { } anchor)
        {
            // To the right of the anchor window, else its left. With room on neither side, take the
            // roomier side flush with the screen edge so the anchor stays as uncovered as possible.
            var gap = (int)(8 * scale);
            var right = area.X + area.Width - (anchor.X + anchor.Width + gap);
            var left = anchor.X - gap - area.X;
            x = right >= size.Width ? anchor.X + anchor.Width + gap
              : left >= size.Width ? anchor.X - gap - size.Width
              : right >= left ? area.X + area.Width - size.Width
              : area.X;
            y = Math.Clamp(anchor.Y, area.Y, area.Y + area.Height - size.Height);
        }
        window.MoveAndResize(new RectInt32(x, y, size.Width, size.Height));
    }

    [DllImport("user32.dll")]
    static extern uint GetDpiForWindow(IntPtr hwnd);
}
