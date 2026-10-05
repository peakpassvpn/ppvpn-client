using Microsoft.UI;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;
using PPVPN.Windows.Platform;
using PPVPN.Windows.Services;
using PPVPN.Windows.Strings;
using Windows.System;
using Windows.UI;

namespace PPVPN.Windows;

/// <summary>
/// The message center: a second window (400×640, min width 340, minimize + close only) with the
/// list and the detail view (<see cref="InboxViewModel"/>). First shown to the right of the main
/// window; its placement is kept.
/// </summary>
public sealed partial class MessageCenterWindow : Window
{
    const string PlacementKey = "messages";

    readonly JsonSettingsStore _settings;
    readonly MainViewModel _main;
    ScrollViewer? _scroller;
    Storyboard? _skeletonPulse;
    bool _closing;
    // A clicked push's read-only detail is showing; when it closes, the list may never have loaded.
    bool _readOnlyDetail;

    public MessageCenterWindow(MainViewModel main, JsonSettingsStore settings, AppWindow? mainWindow)
    {
        _main = main;
        _settings = settings;
        InitializeComponent();
        // Minimize and close only.
        var presenter = OverlappedPresenter.Create();
        presenter.IsMaximizable = false;
        presenter.PreferredMinimumWidth = 340;
        presenter.PreferredMinimumHeight = 400;
        AppWindow.SetPresenter(presenter);
        // The drawn caption buttons follow the window style; drop the maximize box explicitly.
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        SetWindowLongPtr(hwnd, GWL_STYLE, GetWindowLongPtr(hwnd, GWL_STYLE) & ~WS_MAXIMIZEBOX);
        Title = Loc.Get("msgWindowTitle");
        ExtendsContentIntoTitleBar = true;
        SetTitleBar(AppTitleBar);
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Standard;
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico"));
        if (!WindowPlacement.Restore(AppWindow, settings.GetWindowPlacement(PlacementKey)))
        {
            global::Windows.Graphics.RectInt32? anchor = mainWindow is { IsVisible: true }
                ? new(mainWindow.Position.X, mainWindow.Position.Y, mainWindow.Size.Width, mainWindow.Size.Height)
                : null;
            WindowPlacement.SizeAndCenter(AppWindow, 400, 640, anchor);
        }

        AppWindow.Closing += (_, _) =>
        {
            _closing = true;
            SavePlacement();
            Inbox.BackCommand.Execute(null);
        };
        Closed += (_, _) => Inbox.PropertyChanged -= OnInboxChanged;
        Root.ActualThemeChanged += (_, _) => { UpdateCaptionButtons(); RebindRows(); };
        Root.KeyDown += OnKeyDown;
        Messages.Loaded += (_, _) =>
        {
            _scroller = FindScroller(Messages);
            if (_scroller is not null) _scroller.ViewChanged += OnScrolled;
        };
        Inbox.PropertyChanged += OnInboxChanged;
        BuildSkeleton();
        UpdateSkeleton();
        UpdateCaptionButtons();
    }

    public InboxViewModel Inbox => _main.Inbox;

    /// <param name="refresh">Reload the first page (not when a specific message is about to open).</param>
    public void ShowAndFocus(bool refresh = true)
    {
        AppWindow.Show();
        if (AppWindow.Presenter is OverlappedPresenter { State: OverlappedPresenterState.Minimized } presenter) presenter.Restore();
        Activate();
        NativeMethods.SetForegroundWindow(WinRT.Interop.WindowNative.GetWindowHandle(this));
        // After the current call: a notification click asks for the window first and then shows
        // the push's read-only detail, which must not touch the inbox (the list loads on Back).
        if (refresh)
            DispatcherQueue.TryEnqueue(() =>
            {
                if (!Inbox.IsDetailReadOnly) Inbox.RefreshCommand.Execute(null);
            });
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

    public void SavePlacement()
    {
        _settings.SetWindowPlacement(PlacementKey, WindowPlacement.Capture(AppWindow));
        _settings.Save();
    }

    public void CloseForReal()
    {
        if (!_closing) Close();
    }

    // --- formatting helpers (x:Bind) -------------------------------------------

    /// <summary>3px left bar: danger for critical, warning for important, none otherwise.</summary>
    public static Brush BarBrush(StatusTone tone) => tone is StatusTone.Bad or StatusTone.Caution ? Ui.StatusToneBrush(tone) : Ui.Transparent;

    public static Brush IconBrush(StatusTone tone) => Ui.StatusToneBrush(tone);

    public static Brush IconBackground(StatusTone tone) => Ui.ToneSoftBrush(Ui.ToTone(tone));

    public static string SeveritySuffix(bool hasSeverity, string label) => hasSeverity ? " · " + label : "";

    /// <summary>Previous / next at the ends: 35% opacity.</summary>
    public static double EndOpacity(bool enabled) => enabled ? 1 : 0.35;

    /// <summary>Type icon (Segoe Fluent) of a message category.</summary>
    public static string TypeGlyph(MessageCategory category) => category switch
    {
        MessageCategory.SubscriptionExpiring => "", // clock
        MessageCategory.SubscriptionExpired => "",  // warning
        MessageCategory.Billing => "",              // payment card
        MessageCategory.Order => "",                // shopping cart
        MessageCategory.Route => "",                // world
        MessageCategory.Announcement => "",         // announcement
        _ => "",                                     // mail
    };

    // --- list -------------------------------------------------------------------

    void OnMessageClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is InboxItemViewModel item) item.ShowDetailCommand.Execute(null);
    }

    /// <summary>Loads the next page within 24 px of the bottom.</summary>
    void OnScrolled(object? sender, ScrollViewerViewChangedEventArgs e)
    {
        if (_scroller is { } s && s.VerticalOffset >= s.ScrollableHeight - 24) Inbox.LoadMoreCommand.Execute(null);
    }

    void OnInboxChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(InboxViewModel.State) or nameof(InboxViewModel.IsFirstLoad)) UpdateSkeleton();
        if (e.PropertyName == nameof(InboxViewModel.IsDetailReadOnly))
        {
            var left = _readOnlyDetail && !Inbox.IsDetailReadOnly;
            _readOnlyDetail = Inbox.IsDetailReadOnly;
            if (left && !Inbox.IsDetailOpen && !Inbox.IsLoaded && !_closing) Inbox.RefreshCommand.Execute(null);
        }
    }

    void BuildSkeleton()
    {
        var bone = (Style)Root.Resources["Bone"];
        for (var i = 0; i < 6; i++)
        {
            var row = new Grid { Padding = new Thickness(24, 14, 16, 14), ColumnSpacing = 12 };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            row.ColumnDefinitions.Add(new ColumnDefinition());
            row.Children.Add(new Border { Width = 32, Height = 32, CornerRadius = new CornerRadius(16), Style = bone });
            var lines = new StackPanel { Spacing = 8 };
            lines.Children.Add(new Border { Width = 180, Height = 12, Style = bone });
            lines.Children.Add(new Border { Width = 120, Height = 10, Style = bone });
            lines.Children.Add(new Border { Width = 240, Height = 10, Style = bone });
            Grid.SetColumn(lines, 1);
            row.Children.Add(lines);
            Skeleton.Children.Add(row);
        }
    }

    /// <summary>First load: skeleton rows with a breathing animation.</summary>
    void UpdateSkeleton()
    {
        var show = Inbox.IsFirstLoad;
        if (show && _skeletonPulse is null)
        {
            var animation = new DoubleAnimation
            {
                From = 1,
                To = 0.45,
                Duration = new Duration(TimeSpan.FromMilliseconds(700)),
                AutoReverse = true,
                RepeatBehavior = RepeatBehavior.Forever,
                EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut },
            };
            Storyboard.SetTarget(animation, Skeleton);
            Storyboard.SetTargetProperty(animation, "Opacity");
            _skeletonPulse = new Storyboard { Children = { animation } };
            _skeletonPulse.Begin();
        }
        else if (!show && _skeletonPulse is not null)
        {
            _skeletonPulse.Stop();
            _skeletonPulse = null;
        }
    }

    /// <summary>Esc in the detail returns to the list (which keeps its scroll position).</summary>
    void OnKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Escape && Inbox.IsDetailOpen)
        {
            e.Handled = true;
            Inbox.BackCommand.Execute(null);
        }
        else if (e.Key == VirtualKey.F5)
        {
            e.Handled = true;
            Inbox.RefreshCommand.Execute(null);
        }
    }

    void RebindRows()
    {
        Messages.ItemsSource = null;
        Messages.ItemsSource = Inbox.Messages;
        Bindings.Update();
    }

    static ScrollViewer? FindScroller(DependencyObject root)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is ScrollViewer scroller) return scroller;
            if (FindScroller(child) is { } found) return found;
        }
        return null;
    }

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
    }

    const int GWL_STYLE = -16;
    const nint WS_MAXIMIZEBOX = 0x00010000;

    [System.Runtime.InteropServices.DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    static extern nint GetWindowLongPtr(nint hwnd, int index);

    [System.Runtime.InteropServices.DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    static extern nint SetWindowLongPtr(nint hwnd, int index, nint value);
}
