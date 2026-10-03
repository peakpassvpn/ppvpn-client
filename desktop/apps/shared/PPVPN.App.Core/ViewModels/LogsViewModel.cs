using System.Collections.ObjectModel;
using System.Globalization;
using System.Text;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;

namespace PPVPN.App.Core.ViewModels;

public enum LogKind { Client, Core }

public enum LogDay { Today, Yesterday }

/// <summary>An entry of the level filter: the lowest severity shown, and its title.</summary>
public sealed record LogSeverityOption(LogSeverity Severity, string Title)
{
    public override string ToString() => Title;
}

/// <summary>One entry of the log file picker: "客户端 · 今天" + the file name as secondary text.</summary>
public sealed record LogFileOption(LogKind Kind, LogDay Day, string FileName, string Label, string FullPath)
{
    public override string ToString() => Label;
}

/// <summary>
/// The Logs page: the crate's client and core logs × today / yesterday (UTC file dates, as the
/// crate rotates them), following the end of the file: as raw text (<see cref="Lines"/>) and parsed
/// (<see cref="Entries"/>: time, level, source, message, fields; see <see cref="LogParser"/>), with
/// <see cref="VisibleEntries"/> the entries that pass the level filter and the search.
/// Opening a file, changing the filter and a burst that replaces every line each update the
/// collections with one reset (a view updating per item would lay out thousands of times); lines
/// arriving while following are added and trimmed one by one, so a list keeps its realized rows and
/// its scroll position.
/// </summary>
public sealed partial class LogsViewModel : ObservableObject
{
    /// <summary>Lines shown when a file is opened (its tail).</summary>
    public const int InitialLines = 2000;
    /// <summary>Lines kept while following; the oldest are dropped.</summary>
    public const int MaxLines = 2000;
    const int TailBytes = 1024 * 1024;
    static readonly TimeSpan PollInterval = TimeSpan.FromSeconds(1);
    /// <summary>The search applies once typing pauses this long.</summary>
    internal static readonly TimeSpan SearchDelay = TimeSpan.FromMilliseconds(200);

    readonly IAppServices _services;
    readonly ILocalizer _strings;
    readonly TimeProvider _time;
    CancellationTokenSource? _following;
    CancellationTokenSource? _searching;
    string _search = "";
    readonly BulkObservableCollection<string> _lines = [];
    readonly BulkObservableCollection<LogEntry> _entries = [];
    readonly BulkObservableCollection<LogEntry> _visible = [];
    long _offset;
    string _partial = "";

    internal LogsViewModel(string logDirectory, IAppServices services, ILocalizer strings, TimeProvider time)
    {
        LogDirectory = logDirectory;
        _services = services;
        _strings = strings;
        _time = time;
        files = BuildFiles();
        selectedFile = files[0];
        SeverityOptions =
        [
            new(LogSeverity.Trace, strings.Get("logAllLevels")),
            new(LogSeverity.Debug, strings.Get("logDebugUp")),
            new(LogSeverity.Info, strings.Get("logInfoUp")),
            new(LogSeverity.Warn, strings.Get("logWarnUp")),
            new(LogSeverity.Error, strings.Get("logErrorOnly")),
        ];
        selectedSeverity = SeverityOptions[0];
    }

    public string LogDirectory { get; }

    /// <summary>Client/core × today/yesterday, in that order.</summary>
    [ObservableProperty] IReadOnlyList<LogFileOption> files;
    LogFileOption selectedFile;

    /// <summary>
    /// The shown file. A null is ignored (and the current file re-announced): a view's
    /// two-way selection writes one back while its list is being replaced.
    /// </summary>
    public LogFileOption SelectedFile
    {
        get => selectedFile;
        set
        {
            // A null a view writes back is ignored without a notification (re-raising can
            // make a two-way ComboBox write back again).
            if (value is null) return;
            if (!SetProperty(ref selectedFile, value)) return;
            OnPropertyChanged(nameof(DisplayPath));
            Open();
        }
    }
    /// <summary>The footer: the full path of the selected file.</summary>
    public string DisplayPath => SelectedFile.FullPath;

    /// <summary>The visible lines, oldest first; new lines are appended at the end (auto-scroll).</summary>
    public ObservableCollection<string> Lines => _lines;

    /// <summary>The lines parsed, oldest first (a line that continues a message joins its entry).</summary>
    public ObservableCollection<LogEntry> Entries => _entries;

    /// <summary>
    /// The entries shown: at least <see cref="MinimumSeverity"/> (an unparsed line counts as info)
    /// and containing <see cref="SearchText"/> (message, source or a field; case-insensitive).
    /// Updated in place as a few lines arrive; replaced (one reset) when the filter changes.
    /// </summary>
    public ObservableCollection<LogEntry> VisibleEntries => _visible;

    /// <summary>The level filter's choices (<c>logAllLevels</c> … <c>logErrorOnly</c>).</summary>
    public IReadOnlyList<LogSeverityOption> SeverityOptions { get; }

    LogSeverityOption selectedSeverity;

    /// <summary>The level filter, two-way; a null a view writes back is ignored.</summary>
    public LogSeverityOption SelectedSeverity
    {
        get => selectedSeverity;
        set
        {
            // A null a view writes back is ignored without a notification (re-raising can
            // make a two-way ComboBox write back again).
            if (value is null) return;
            if (!SetProperty(ref selectedSeverity, value)) return;
            OnPropertyChanged(nameof(MinimumSeverity));
            Refilter();
        }
    }

    /// <summary>The lowest severity shown.</summary>
    public LogSeverity MinimumSeverity
    {
        get => SelectedSeverity.Severity;
        set => SelectedSeverity = SeverityOptions.FirstOrDefault(o => o.Severity == value) ?? SeverityOptions[0];
    }

    /// <summary>
    /// The search box (<c>logSearch</c> placeholder); empty shows everything. Applied once typing
    /// pauses for <see cref="SearchDelay"/>, so each keystroke does not refilter the list.
    /// </summary>
    [ObservableProperty] string searchText = "";

    partial void OnSearchTextChanged(string value) => ApplySearchSoon();

    async void ApplySearchSoon()
    {
        _searching?.Cancel();
        var cts = new CancellationTokenSource();
        _searching = cts;
        try
        {
            await Task.Delay(SearchDelay, _time, cts.Token);
        }
        catch (OperationCanceledException)
        {
            return;
        }
        if (_searching == cts) ApplySearch();
    }

    /// <summary>Apply <see cref="SearchText"/> now (after the pause; tests call it directly).</summary>
    internal void ApplySearch()
    {
        _searching?.Cancel();
        _searching = null;
        var search = SearchText.Trim();
        if (search == _search) return;
        _search = search;
        Refilter();
    }

    /// <summary>Entries exist but the filter hides them all (<c>logNoMatches</c>).</summary>
    public bool HasNoMatches => VisibleEntries.Count == 0 && Entries.Count > 0;

    /// <summary><c>copyShownLogs</c>: the shown entries' original text.</summary>
    [RelayCommand]
    void CopyVisible() => _services.CopyText(string.Join("\n", VisibleEntries.Select(e => e.Raw)));

    /// <summary>The page follows the end of the file (green breathing dot + <c>following</c>).</summary>
    [ObservableProperty] bool isFollowing;
    /// <summary>The selected file does not exist (yet).</summary>
    [ObservableProperty] bool isMissing;

    /// <summary>Raised after lines were appended (views scroll to the end).</summary>
    public event Action? LinesAppended;

    /// <summary><c>revealWin</c> / <c>revealLinux</c>: open the log folder.</summary>
    [RelayCommand]
    void OpenFolder()
    {
        Directory.CreateDirectory(LogDirectory);
        _services.OpenFolder(LogDirectory);
    }

    /// <summary>Reload the selected file from its tail (F5).</summary>
    [RelayCommand]
    void Reload() => Open();

    /// <summary>Start following while the page is visible.</summary>
    public async void Start()
    {
        _following?.Cancel();
        var cts = new CancellationTokenSource();
        _following = cts;
        // The date may have changed since the page was last shown.
        var selected = SelectedFile;
        var rebuilt = BuildFiles();
        // Replaced only when the date changed: a new list resets the views' selection.
        if (!rebuilt.SequenceEqual(Files)) Files = rebuilt;
        var reselect = Files.First(f => f.Kind == selected.Kind && f.Day == selected.Day);
        if (reselect != SelectedFile) SelectedFile = reselect;
        else Open();
        IsFollowing = true;
        try
        {
            while (!cts.IsCancellationRequested)
            {
                await Task.Delay(PollInterval, _time, cts.Token);
                Poll();
            }
        }
        catch (OperationCanceledException) { }
        finally
        {
            if (_following == cts) IsFollowing = false;
        }
    }

    public void Stop()
    {
        _following?.Cancel();
        IsFollowing = false;
    }

    IReadOnlyList<LogFileOption> BuildFiles()
    {
        var today = _time.GetUtcNow().UtcDateTime.Date;
        var list = new List<LogFileOption>();
        foreach (var kind in new[] { LogKind.Client, LogKind.Core })
        foreach (var day in new[] { LogDay.Today, LogDay.Yesterday })
        {
            var date = day == LogDay.Today ? today : today.AddDays(-1);
            var name = $"ppvpn-{(kind == LogKind.Client ? "client" : "core")}.{date.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture)}.log";
            var label = $"{_strings.Get(kind == LogKind.Client ? "client" : "core")} · {_strings.Get(day == LogDay.Today ? "today" : "yesterday")}";
            list.Add(new LogFileOption(kind, day, name, label, Path.Combine(LogDirectory, name)));
        }
        return list;
    }

    /// <summary>Load the tail of the selected file.</summary>
    internal void Open()
    {
        _partial = "";
        _offset = 0;
        List<string> lines = [];
        var path = SelectedFile.FullPath;
        IsMissing = !File.Exists(path);
        if (!IsMissing)
        {
            try
            {
                using var stream = OpenShared(path);
                var start = Math.Max(0, stream.Length - TailBytes);
                stream.Seek(start, SeekOrigin.Begin);
                var text = ReadAll(stream);
                _offset = stream.Length;
                if (start > 0 && text.IndexOf('\n') is var cut and >= 0) text = text[(cut + 1)..];
                lines = Split(text);
                if (lines.Count > InitialLines) lines = lines.GetRange(lines.Count - InitialLines, InitialLines);
            }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }
        var entries = new List<LogEntry>(lines.Count);
        var zone = _time.LocalTimeZone;
        foreach (var line in lines) LogParser.Append(entries, line, zone);
        Replace(lines, entries);
        LinesAppended?.Invoke();
    }

    /// <summary>Append what was written since the last read (tests call it directly).</summary>
    internal void Poll()
    {
        var path = SelectedFile.FullPath;
        if (!File.Exists(path))
        {
            if (!IsMissing) Open();
            return;
        }
        if (IsMissing)
        {
            Open();
            return;
        }
        try
        {
            using var stream = OpenShared(path);
            if (stream.Length < _offset)
            {
                // Truncated or replaced: start over.
                Open();
                return;
            }
            if (stream.Length == _offset) return;
            stream.Seek(_offset, SeekOrigin.Begin);
            var text = ReadAll(stream);
            _offset = stream.Length;
            var lines = Split(text);
            if (lines.Count == 0) return;
            Append(lines);
            LinesAppended?.Invoke();
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    /// <summary>
    /// Appends raw lines and their parsed entries (the visible ones filtered as they come) one by
    /// one, then drops the oldest past <see cref="MaxLines"/> one by one. A reset made the list
    /// re-virtualize every poll while following: hitches, and a scrolled-up view could jump. Only
    /// a burst of <see cref="MaxLines"/> or more, which replaces every line, is one reset.
    /// </summary>
    void Append(List<string> lines)
    {
        // Parsed after the last entry, which a line may continue.
        var zone = _time.LocalTimeZone;
        var last = _entries.Count > 0 ? _entries[^1] : null;
        var parsed = new List<LogEntry>(lines.Count + 1);
        if (last is not null) parsed.Add(last);
        foreach (var line in lines) LogParser.Append(parsed, line, zone);
        var continued = last is not null && !ReferenceEquals(parsed[0], last);
        var added = last is null ? parsed : parsed.GetRange(1, parsed.Count - 1);

        if (lines.Count >= MaxLines)
        {
            var allLines = _lines.Concat(lines).ToList();
            var allEntries = _entries.Take(_entries.Count - (last is null ? 0 : 1)).Concat(parsed).ToList();
            Replace(
                allLines.Count > MaxLines ? allLines.GetRange(allLines.Count - MaxLines, MaxLines) : allLines,
                allEntries.Count > MaxLines ? allEntries.GetRange(allEntries.Count - MaxLines, MaxLines) : allEntries);
            return;
        }

        foreach (var line in lines) _lines.Add(line);
        if (continued)
        {
            // The last entry grew: its visible copy follows.
            var entry = parsed[0];
            _entries[^1] = entry;
            var index = _visible.Count - 1;
            var wasVisible = index >= 0 && ReferenceEquals(_visible[index], last);
            var matches = Matches(entry);
            if (wasVisible && matches) _visible[index] = entry;
            else if (wasVisible) _visible.RemoveAt(index);
            else if (matches) _visible.Add(entry);
        }
        foreach (var entry in added)
        {
            _entries.Add(entry);
            if (Matches(entry)) _visible.Add(entry);
        }
        while (_lines.Count > MaxLines) _lines.RemoveAt(0);
        while (_entries.Count > MaxLines)
        {
            var first = _entries[0];
            _entries.RemoveAt(0);
            if (_visible.Count > 0 && ReferenceEquals(_visible[0], first)) _visible.RemoveAt(0);
        }
        OnPropertyChanged(nameof(HasNoMatches));
    }

    /// <summary>Replaces the lines, the entries and the visible entries, one reset each.</summary>
    void Replace(List<string> lines, List<LogEntry> entries)
    {
        _lines.ReplaceAll(lines);
        _entries.ReplaceAll(entries);
        _visible.ReplaceAll(entries.Where(Matches));
        OnPropertyChanged(nameof(HasNoMatches));
    }

    bool Matches(LogEntry entry)
    {
        var severity = entry.Severity == LogSeverity.Unknown ? LogSeverity.Info : entry.Severity;
        if (severity < MinimumSeverity) return false;
        var search = _search;
        if (search.Length == 0) return true;
        return entry.Message.Contains(search, StringComparison.OrdinalIgnoreCase)
            || (entry.Source?.Contains(search, StringComparison.OrdinalIgnoreCase) ?? false)
            || entry.Fields.Any(f => f.Key.Contains(search, StringComparison.OrdinalIgnoreCase)
                || f.Value.Contains(search, StringComparison.OrdinalIgnoreCase));
    }

    void Refilter()
    {
        _visible.ReplaceAll(_entries.Where(Matches));
        OnPropertyChanged(nameof(HasNoMatches));
    }

    static FileStream OpenShared(string path) =>
        new(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);

    static string ReadAll(Stream stream)
    {
        using var reader = new StreamReader(stream, Encoding.UTF8, detectEncodingFromByteOrderMarks: false, leaveOpen: true);
        return reader.ReadToEnd();
    }

    /// <summary>Complete lines of <paramref name="text"/> (after the carried-over partial line); keeps a trailing partial line.</summary>
    List<string> Split(string text)
    {
        var all = (_partial + text).Split('\n');
        _partial = all[^1];
        return all[..^1].Select(line => line.TrimEnd('\r')).ToList();
    }
}
