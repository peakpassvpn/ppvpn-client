using System.Collections.Specialized;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Animation;
using Microsoft.UI.Xaml.Navigation;
using PPVPN.App.Core.ViewModels;
using PPVPN.Windows.Strings;

namespace PPVPN.Windows.Views;

/// <summary>
/// One crate log (client or core × today or yesterday, UTC file dates), parsed by App.Core into
/// entries: a virtualized list of <see cref="LogsViewModel.VisibleEntries"/> (time, level badge,
/// source, message, fields dimmed), filtered by level and search, scrolled to the end while
/// following.
/// </summary>
public sealed partial class LogsPage : Page
{
    /// <summary>How long after a wheel, pointer (mouse, pen, touch) or key input a settled view change counts as the user's.</summary>
    const long UserScrollWindowMs = 1000;
    long _userInputAt = long.MinValue / 2; // "long ago" without overflowing the subtraction below
    /// <summary>Re-scrolls in a row while following, when a layout pass left the view short of the end.</summary>
    int _chases;
    bool _scrollQueued;
    Storyboard? _breathing;
    ScrollViewer? _scroller;

    public LogsPage()
    {
        InitializeComponent();
        ViewModel.VisibleEntries.CollectionChanged += OnEntriesChanged;
        ViewModel.LinesAppended += OnLinesAppended;
        ViewModel.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName == nameof(LogsViewModel.IsFollowing)) UpdateFollow();
        };
        EntryList.Loaded += (_, _) =>
        {
            if (_scroller is not null) return;
            _scroller = FindScroller(EntryList);
            if (_scroller is not null)
            {
                _scroller.ViewChanged += OnViewChanged;
                // Only the user's own input may stop or resume following (handled events too: the
                // ScrollViewer and the list consume them).
                _scroller.AddHandler(PointerWheelChangedEvent, new PointerEventHandler(OnUserScrollInput), true);
                _scroller.AddHandler(PointerPressedEvent, new PointerEventHandler(OnUserScrollInput), true);
                _scroller.AddHandler(PointerReleasedEvent, new PointerEventHandler(OnUserScrollInput), true);
                EntryList.AddHandler(KeyDownEvent, new KeyEventHandler((_, _) => MarkUserScroll()), true);
            }
            if (ViewModel.IsFollowing) ScrollToEnd();
        };
    }

    /// <summary>Debug / trace entries read dimmed.</summary>
    public static double EntryOpacity(bool dim) => dim ? 0.6 : 1;

    /// <summary>Level badge colour: error red, warning amber, the rest the neutral text colour.</summary>
    public static Brush ToneBrush(StatusTone tone) => tone == StatusTone.Neutral
        ? (Brush)Application.Current.Resources["TextFillColorSecondaryBrush"]
        : Ui.ToneBrush(Ui.ToTone(tone));

    /// <summary>The fields after the message: "  key=value …", or nothing.</summary>
    public static string FieldsSuffix(string fields) => string.IsNullOrEmpty(fields) ? "" : "  " + fields;

    public LogsViewModel ViewModel => App.ViewModel.Logs;

    protected override void OnNavigatedTo(NavigationEventArgs e)
    {
        base.OnNavigatedTo(e);
        ViewModel.Start();
        UpdateFollow();
    }

    protected override void OnNavigatedFrom(NavigationEventArgs e)
    {
        base.OnNavigatedFrom(e);
        ViewModel.Stop();
        _breathing?.Stop();
        _breathing = null;
    }

    /// <summary>A new entry at the end while following: keep the end in view.</summary>
    void OnEntriesChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        if (ViewModel.IsFollowing && e.Action is NotifyCollectionChangedAction.Add or NotifyCollectionChangedAction.Reset) QueueScrollToEnd();
    }

    void OnLinesAppended()
    {
        if (ViewModel.IsFollowing) QueueScrollToEnd();
    }

    /// <summary>
    /// Scrolls to the end once the current batch of changes is in: each ScrollIntoView measures
    /// the wrapped rows, so one per added entry stalled the UI thread on a long file.
    /// </summary>
    void QueueScrollToEnd()
    {
        if (_scrollQueued) return;
        _scrollQueued = DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            _scrollQueued = false;
            if (ViewModel.IsFollowing) ScrollToEnd();
        });
    }

    void ScrollToEnd()
    {
        if (ViewModel.VisibleEntries.Count == 0) return;
        EntryList.ScrollIntoView(ViewModel.VisibleEntries[^1], ScrollIntoViewAlignment.Leading);
    }

    void OnUserScrollInput(object sender, PointerRoutedEventArgs e) => MarkUserScroll();

    void MarkUserScroll()
    {
        _userInputAt = Environment.TickCount64;
        _chases = 0;
    }

    /// <summary>
    /// The user scrolling up stops following; scrolling back to the end resumes it. Our own scrolls,
    /// resets and growing content never change following (a flag set per ScrollIntoView went stale
    /// when that raised no ViewChanged and then swallowed the user's next scroll); while following,
    /// a view a layout pass left short of the end is scrolled again.
    /// </summary>
    void OnViewChanged(object? sender, ScrollViewerViewChangedEventArgs e)
    {
        if (e.IsIntermediate || _scroller is null) return;
        var atEnd = _scroller.VerticalOffset >= _scroller.ScrollableHeight - 4;
        if (Environment.TickCount64 - _userInputAt <= UserScrollWindowMs)
        {
            if (ViewModel.IsFollowing && !atEnd) ViewModel.IsFollowing = false;
            else if (!ViewModel.IsFollowing && atEnd && _scroller.ScrollableHeight > 0) ViewModel.IsFollowing = true;
            return;
        }
        if (!ViewModel.IsFollowing || atEnd)
        {
            _chases = 0;
            return;
        }
        if (_chases++ < 3) QueueScrollToEnd();
    }

    static ScrollViewer? FindScroller(DependencyObject root)
    {
        for (var i = 0; i < VisualTreeHelper.GetChildrenCount(root); i++)
        {
            var child = VisualTreeHelper.GetChild(root, i);
            if (child is ScrollViewer viewer) return viewer;
            if (FindScroller(child) is { } found) return found;
        }
        return null;
    }

    /// <summary>"%LOCALAPPDATA%\PPVPN\logs\ppvpn-client.2026-09-30.log" (design: the path footer).</summary>
    public static string ShortPath(string path)
    {
        var local = Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData);
        return path.StartsWith(local, StringComparison.OrdinalIgnoreCase) ? "%LOCALAPPDATA%" + path[local.Length..] : path;
    }

    /// <summary>Row menu › Copy line: the entry's original text, whole (the row shows at most <see cref="LogEntry.DisplayLimit"/> characters).</summary>
    void OnCopyLine(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { Tag: LogEntry entry }) App.Services.CopyText(entry.Raw);
    }

    void OnFollowClick(object sender, RoutedEventArgs e)
    {
        ViewModel.IsFollowing = !ViewModel.IsFollowing;
        if (ViewModel.IsFollowing) ScrollToEnd();
    }

    /// <summary>Breathing success dot (1.6 s) while following.</summary>
    void UpdateFollow()
    {
        var following = ViewModel.IsFollowing;
        FollowText.Text = Loc.Get(following ? "following" : "followPaused");
        FollowDot.Opacity = 1;
        FollowDot.Fill = Ui.ToneBrush(following ? ConnectionTone.Ok : ConnectionTone.Idle);
        if (following && _breathing is null)
        {
            var animation = new DoubleAnimation
            {
                From = 1,
                To = 0.3,
                Duration = new Duration(TimeSpan.FromMilliseconds(800)),
                AutoReverse = true,
                RepeatBehavior = RepeatBehavior.Forever,
                EasingFunction = new SineEase { EasingMode = EasingMode.EaseInOut },
            };
            Storyboard.SetTarget(animation, FollowDot);
            Storyboard.SetTargetProperty(animation, "Opacity");
            _breathing = new Storyboard { Children = { animation } };
            _breathing.Begin();
        }
        else if (!following && _breathing is not null)
        {
            _breathing.Stop();
            _breathing = null;
        }
    }
}
