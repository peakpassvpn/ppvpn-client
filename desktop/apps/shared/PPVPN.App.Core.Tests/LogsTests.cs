using System.Collections.Specialized;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.App.Core.Tests;

public sealed class LogsTests
{
    [Fact]
    public async Task FilesAreClientAndCoreForTodayAndYesterdayInUtc()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // 01:30 in UTC+8 on the 30th is still the 29th in UTC.
            var time = new ManualTime(new DateTimeOffset(2026, 9, 30, 1, 30, 0, TimeSpan.FromHours(8)));
            var t = new Scripted(time: time);
            var logs = t.Main.Logs;
            Assert.Equal(
                ["ppvpn-client.2026-09-29.log", "ppvpn-client.2026-09-28.log", "ppvpn-core.2026-09-29.log", "ppvpn-core.2026-09-28.log"],
                logs.Files.Select(f => f.FileName));
            Assert.Equal(["client · today", "client · yesterday", "core · today", "core · yesterday"], logs.Files.Select(f => f.Label));
            Assert.Equal(Path.Combine(logs.LogDirectory, "ppvpn-client.2026-09-29.log"), logs.DisplayPath);
            await Task.CompletedTask;
        });
    }

    [Fact]
    public async Task TailThenFollowTheEnd()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var time = new ManualTime(DateTimeOffset.UtcNow);
            var t = new Scripted(time: time);
            var logs = t.Main.Logs;
            Directory.CreateDirectory(logs.LogDirectory);
            var path = logs.Files[2].FullPath; // core · today
            File.WriteAllLines(path, Enumerable.Range(1, LogsViewModel.InitialLines + 100).Select(i => $"line {i}"));
            try
            {
                logs.SelectedFile = logs.Files[2];
                Assert.False(logs.IsMissing);
                Assert.Equal(LogsViewModel.InitialLines, logs.Lines.Count);
                Assert.Equal("line 101", logs.Lines[0]);
                Assert.Equal($"line {LogsViewModel.InitialLines + 100}", logs.Lines[^1]);

                var appended = 0;
                logs.LinesAppended += () => appended++;
                // A partial line waits for its newline.
                File.AppendAllText(path, "line 301\nline 30");
                logs.Poll();
                Assert.Equal("line 301", logs.Lines[^1]);
                File.AppendAllText(path, "2\n");
                logs.Poll();
                Assert.Equal("line 302", logs.Lines[^1]);
                Assert.Equal(2, appended);

                // Truncated (rotated or cleared): start over.
                File.WriteAllText(path, "fresh\n");
                logs.Poll();
                Assert.Equal(["fresh"], logs.Lines);

                // A missing file.
                logs.SelectedFile = logs.Files[3];
                Assert.True(logs.IsMissing);
                Assert.Empty(logs.Lines);
            }
            finally
            {
                File.Delete(path);
            }
            await Task.CompletedTask;
        });
    }

    [Fact]
    public async Task ANullSelectionFromAViewIsIgnored()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted(time: new ManualTime(DateTimeOffset.UtcNow));
            var logs = t.Main.Logs;
            logs.SelectedFile = logs.Files[2];
            var files = logs.Files;
            var changed = new List<string?>();
            logs.PropertyChanged += (_, e) => changed.Add(e.PropertyName);

            // What a ComboBox's two-way binding writes while its ItemsSource is swapped.
            logs.SelectedFile = null!;
            Assert.Equal(files[2], logs.SelectedFile);
            // Ignored without a notification (re-raising made a two-way ComboBox write back again).
            Assert.DoesNotContain(nameof(LogsViewModel.SelectedFile), changed);

            // Showing the page again on the same day keeps the list (and so the selection).
            logs.Start();
            logs.Stop();
            Assert.Same(files, logs.Files);
            Assert.Equal(files[2], logs.SelectedFile);
            await Task.CompletedTask;
        });
    }

    [Fact]
    public async Task EntriesAreParsedFilteredAndFollowTheFile()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted(time: new ManualTime(DateTimeOffset.UtcNow));
            var logs = t.Main.Logs;
            Directory.CreateDirectory(logs.LogDirectory);
            var path = logs.Files[0].FullPath; // client · today
            File.WriteAllLines(path,
            [
                "2026-10-01T01:00:00Z  INFO ppvpn_client::a: connected generation=1",
                "2026-10-01T01:00:01Z DEBUG ppvpn_client::a: probing node=hk1",
                "2026-10-01T01:00:02Z  WARN ppvpn_client::a: slow health check took_ms=4000",
            ]);
            try
            {
                logs.SelectedFile = logs.Files[1];
                logs.SelectedFile = logs.Files[0];
                Assert.Equal(3, logs.Lines.Count);
                Assert.Equal(3, logs.Entries.Count);
                Assert.Equal(3, logs.VisibleEntries.Count);
                Assert.Equal([LogSeverity.Info, LogSeverity.Debug, LogSeverity.Warn], logs.Entries.Select(e => e.Severity));

                // Level filter: info and above hides the debug line.
                Assert.Equal(LogSeverity.Trace, logs.MinimumSeverity);
                logs.SelectedSeverity = logs.SeverityOptions.Single(o => o.Severity == LogSeverity.Info);
                Assert.Equal(["connected", "slow health check"], logs.VisibleEntries.Select(e => e.Message));
                logs.SelectedSeverity = null!; // a view's two-way binding writing back null
                Assert.Equal(LogSeverity.Info, logs.MinimumSeverity);

                // Search: message, source or fields, case-insensitive (applied after a pause; see below).
                logs.SearchText = "TOOK_MS"; logs.ApplySearch();
                Assert.Equal(["slow health check"], logs.VisibleEntries.Select(e => e.Message));
                logs.SearchText = "nothing like this"; logs.ApplySearch();
                Assert.True(logs.HasNoMatches);
                logs.SearchText = ""; logs.ApplySearch();
                Assert.False(logs.HasNoMatches);

                // Appended lines are parsed and filtered as they come; a continuation joins its entry.
                File.AppendAllText(path,
                    "2026-10-01T01:00:03Z ERROR ppvpn_client::a: panic: boom\nstack backtrace:\n" +
                    "2026-10-01T01:00:04Z DEBUG ppvpn_client::a: hidden\n");
                logs.Poll();
                Assert.Equal(6, logs.Lines.Count);
                Assert.Equal(5, logs.Entries.Count);
                Assert.Equal("panic: boom\nstack backtrace:", logs.Entries[3].Message);
                Assert.Equal(["connected", "slow health check", "panic: boom\nstack backtrace:"],
                    logs.VisibleEntries.Select(e => e.Message));

                // Copy: the shown entries' original text.
                logs.CopyVisibleCommand.Execute(null);
                Assert.Equal(
                    "2026-10-01T01:00:00Z  INFO ppvpn_client::a: connected generation=1\n" +
                    "2026-10-01T01:00:02Z  WARN ppvpn_client::a: slow health check took_ms=4000\n" +
                    "2026-10-01T01:00:03Z ERROR ppvpn_client::a: panic: boom\nstack backtrace:",
                    t.Services.Copied.Single());
            }
            finally
            {
                File.Delete(path);
            }
            await Task.CompletedTask;
        });
    }

    [Fact]
    public async Task LongFilesUpdateTheListInBulk()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted(time: new ManualTime(DateTimeOffset.UtcNow));
            var logs = t.Main.Logs;
            Directory.CreateDirectory(logs.LogDirectory);
            var path = logs.Files[0].FullPath; // client · today
            static string Line(int i) => $"2026-10-01T01:00:00Z  INFO ppvpn_client::a: line {i} n={i}";
            File.WriteAllLines(path, Enumerable.Range(1, 5000).Select(Line));
            var changes = new List<NotifyCollectionChangedAction>();
            logs.VisibleEntries.CollectionChanged += (_, e) => changes.Add(e.Action);
            try
            {
                // Opening: one reset, not one notification per entry.
                logs.SelectedFile = logs.Files[1];
                logs.SelectedFile = logs.Files[0];
                Assert.Equal(LogsViewModel.InitialLines, logs.VisibleEntries.Count);
                Assert.Equal("line 5000", logs.VisibleEntries[^1].Message);
                Assert.True(changes.Count <= 2, $"{changes.Count} notifications");
                Assert.All(changes, a => Assert.Equal(NotifyCollectionChangedAction.Reset, a));

                // A few new lines are added one by one, and as many of the oldest dropped.
                changes.Clear();
                File.AppendAllLines(path, Enumerable.Range(5001, 3).Select(Line));
                logs.Poll();
                Assert.Equal(
                    [NotifyCollectionChangedAction.Add, NotifyCollectionChangedAction.Add, NotifyCollectionChangedAction.Add,
                     NotifyCollectionChangedAction.Remove, NotifyCollectionChangedAction.Remove, NotifyCollectionChangedAction.Remove],
                    changes);

                // Following: new lines are added and the oldest dropped one by one, never a reset
                // (a reset re-virtualizes the list and can move a scrolled-up view).
                changes.Clear();
                File.AppendAllLines(path, Enumerable.Range(5004, 1000).Select(Line));
                logs.Poll();
                var next = 6004;
                for (var poll = 0; poll < 30; poll++)
                {
                    File.AppendAllLines(path, Enumerable.Range(next, 50).Select(Line));
                    next += 50;
                    logs.Poll();
                }
                Assert.DoesNotContain(NotifyCollectionChangedAction.Reset, changes);
                Assert.Equal(1000 + 30 * 50, changes.Count(a => a == NotifyCollectionChangedAction.Add));
                Assert.Equal(1000 + 30 * 50, changes.Count(a => a == NotifyCollectionChangedAction.Remove));
                Assert.Equal(LogsViewModel.MaxLines, logs.Entries.Count);
                Assert.Equal(LogsViewModel.MaxLines, logs.Lines.Count);
                Assert.Equal(LogsViewModel.MaxLines, logs.VisibleEntries.Count);
                Assert.Equal($"line {next - 1}", logs.VisibleEntries[^1].Message);
                Assert.Equal($"line {next - LogsViewModel.MaxLines}", logs.VisibleEntries[0].Message);

                // A burst that replaces every line is one reset.
                changes.Clear();
                File.AppendAllLines(path, Enumerable.Range(next, LogsViewModel.MaxLines + 10).Select(Line));
                next += LogsViewModel.MaxLines + 10;
                logs.Poll();
                Assert.Equal([NotifyCollectionChangedAction.Reset], changes);
                Assert.Equal(LogsViewModel.MaxLines, logs.Entries.Count);
                Assert.Equal($"line {next - 1}", logs.VisibleEntries[^1].Message);

                // Narrowing the level and widening it back: one reset each.
                changes.Clear();
                logs.SelectedSeverity = logs.SeverityOptions.Single(o => o.Severity == LogSeverity.Warn);
                Assert.Empty(logs.VisibleEntries);
                logs.SelectedSeverity = logs.SeverityOptions[0];
                Assert.Equal(logs.Entries.Count, logs.VisibleEntries.Count);
                Assert.Equal([NotifyCollectionChangedAction.Reset, NotifyCollectionChangedAction.Reset], changes);

                // Typing in the search box refilters once, after the pause.
                changes.Clear();
                foreach (var text in new[] { "l", "li", "lin", "line 90" }) logs.SearchText = text;
                Assert.Empty(changes);
                await Task.Delay(LogsViewModel.SearchDelay * 4);
                Assert.Equal([NotifyCollectionChangedAction.Reset], changes);
                Assert.All(logs.VisibleEntries, e => Assert.Contains("line 90", e.Message));
                Assert.NotEmpty(logs.VisibleEntries);
            }
            finally
            {
                File.Delete(path);
            }
        });
    }

    [Fact]
    public void AnEndlessMessageIsCutForDisplayButCopiedWhole()
    {
        var message = new string('x', LogEntry.DisplayLimit + 10);
        var entry = new LogEntry(null, LogSeverity.Unknown, null, message, [], message, "");
        Assert.Equal(LogEntry.DisplayLimit + 2, entry.DisplayMessage.Length);
        Assert.Same(entry.Raw, message);
        var brief = new LogEntry(null, LogSeverity.Unknown, null, "short", [], "short", "");
        Assert.Same(brief.Message, brief.DisplayMessage);
    }
}
