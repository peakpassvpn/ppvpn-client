using System.Text;
using PPVPN.App.Core.ViewModels;
using static PPVPN.Linux.UI.L;
using static PPVPN.Linux.UI.Widgets;

namespace PPVPN.Linux.UI;

/// <summary>
/// The client and core logs, today or yesterday, parsed: each entry is its time (monospace, dim),
/// a severity label in its tone, the source (small), the message and its fields (dim
/// <c>key=value</c>). A level filter and a search box narrow what is shown; new entries are
/// appended as they arrive.
/// </summary>
public static class LogsPage
{
    public static Gtk.Widget Create(LogsViewModel vm)
    {
        var root = Gtk.Box.New(Gtk.Orientation.Vertical, 0);

        var bar = Gtk.Box.New(Gtk.Orientation.Horizontal, 8);
        bar.SetSizeRequest(-1, 46);
        bar.SetMarginStart(12);
        bar.SetMarginEnd(12);
        bar.Append(Label(T("logFile"), "dim-label"));
        var files = Gtk.DropDown.NewFromStrings([]);
        files.SetValign(Gtk.Align.Center);
        var updatingFiles = false;
        files.OnNotify += (_, args) =>
        {
            if (updatingFiles || args.Pspec.GetName() != "selected") return;
            var index = (int)files.GetSelected();
            if (index >= 0 && index < vm.Files.Count) vm.SelectedFile = vm.Files[index];
        };
        bar.Append(files);
        var following = Gtk.Box.New(Gtk.Orientation.Horizontal, 6);
        following.SetValign(Gtk.Align.Center);
        following.Append(Label("●", "ppvpn-live-dot"));
        following.Append(Label(T("following"), "dim-label"));
        bar.Append(following);

        // Level filter and search.
        var severity = Gtk.DropDown.NewFromStrings(vm.SeverityOptions.Select(o => o.Title).ToArray());
        severity.SetValign(Gtk.Align.Center);
        var updatingSeverity = false;
        severity.OnNotify += (_, args) =>
        {
            if (updatingSeverity || args.Pspec.GetName() != "selected") return;
            var index = (int)severity.GetSelected();
            if (index >= 0 && index < vm.SeverityOptions.Count) vm.SelectedSeverity = vm.SeverityOptions[index];
        };
        bar.Append(severity);
        var search = Gtk.SearchEntry.New();
        search.SetValign(Gtk.Align.Center);
        // Takes the free width, so a longer level name does not widen the window.
        search.SetHexpand(true);
        search.SetSizeRequest(120, -1);
        // The property, not gtk_search_entry_set_placeholder_text (GTK 4.10; jammy has 4.6).
        Gtk.SearchEntry.PlaceholderTextPropertyDefinition.Set(search, T("logSearch"));
        search.OnSearchChanged += (_, _) => vm.SearchText = search.GetText();
        bar.Append(search);
        var copy = IconButton("edit-copy-symbolic", T("copyShownLogs"), () => vm.CopyVisibleCommand.Execute());
        bar.Append(copy);
        bar.Append(TextButton(T("revealLinux"), () => vm.OpenFolderCommand.Execute()));
        root.Append(bar);

        var stack = Gtk.Stack.New();
        stack.SetVexpand(true);
        var missing = Adw.StatusPage.New();
        missing.SetIconName("text-x-generic-symbolic");
        stack.AddNamed(missing, "missing");
        var noMatches = Adw.StatusPage.New();
        noMatches.SetIconName("edit-find-symbolic");
        noMatches.SetTitle(T("logNoMatches"));
        stack.AddNamed(noMatches, "empty");

        var view = Gtk.TextView.New();
        view.SetEditable(false);
        view.SetCursorVisible(false);
        view.SetWrapMode(Gtk.WrapMode.WordChar);
        view.AddCssClass("ppvpn-log");
        view.SetTopMargin(10);
        view.SetBottomMargin(10);
        view.SetLeftMargin(12);
        view.SetRightMargin(12);
        view.SetPixelsBelowLines(2);
        var scroller = Gtk.ScrolledWindow.New();
        scroller.SetChild(view);
        scroller.SetVexpand(true);
        stack.AddNamed(scroller, "log");
        root.Append(stack);

        var path = Label(null, "ppvpn-weakest", "ppvpn-mono");
        path.SetMarginStart(12);
        path.SetMarginTop(6);
        path.SetMarginBottom(8);
        path.SetSelectable(true);
        path.SetEllipsize(Pango.EllipsizeMode.Middle);
        root.Append(path);

        var buffer = view.GetBuffer();
        var adjustment = scroller.GetVadjustment();
        bool AtEnd() => adjustment.GetValue() >= adjustment.GetUpper() - adjustment.GetPageSize() - 8;
        // Follow the end until the reader scrolls up away from it, and again once they are back
        // at the end. Entries arrive before the first layout and the text view lays out lazily
        // (its scroll-to-mark lands short), so the end is re-applied whenever the height grows;
        // while that layout settles the view also moves itself back up (keeping its first line),
        // which is undone rather than taken for the reader scrolling.
        var followEnd = true;
        var lastValue = 0.0;
        var settleUntil = DateTime.MinValue;
        adjustment.OnValueChanged += (_, _) =>
        {
            if (followEnd && DateTime.UtcNow < settleUntil && !AtEnd() && adjustment.GetPageSize() > 0)
            {
                adjustment.SetValue(adjustment.GetUpper() - adjustment.GetPageSize());
                lastValue = adjustment.GetValue();
                return;
            }
            var value = adjustment.GetValue();
            if (value < lastValue - 1) followEnd = AtEnd();
            else if (AtEnd()) followEnd = true;
            lastValue = value;
        };
        adjustment.OnChanged += (_, _) =>
        {
            if (followEnd && adjustment.GetPageSize() > 0) adjustment.SetValue(adjustment.GetUpper() - adjustment.GetPageSize());
        };
        void ScrollToEnd()
        {
            if (adjustment.GetPageSize() > 0) adjustment.SetValue(adjustment.GetUpper() - adjustment.GetPageSize());
        }
        // What the buffer shows, in order, and how many text lines each entry takes (a message
        // can span lines), so a trim at the top deletes just those lines.
        var shown = new List<LogEntry>();
        var shownLines = new List<int>();
        void Append(IReadOnlyList<LogEntry> entries)
        {
            if (entries.Count == 0) return;
            var markup = new StringBuilder();
            foreach (var entry in entries)
            {
                if (markup.Length > 0 || buffer.GetCharCount() > 0) markup.Append('\n');
                var text = Markup(entry);
                markup.Append(text);
                shown.Add(entry);
                shownLines.Add(1 + text.Count(c => c == '\n'));
            }
            buffer.GetEndIter(out var end);
            buffer.InsertMarkup(end, markup.ToString(), -1);
        }
        void Followed()
        {
            if (!followEnd) return;
            settleUntil = DateTime.UtcNow.AddSeconds(1);
            ScrollToEnd();
        }
        // A file or filter change opens at the end.
        void Rebuild()
        {
            followEnd = true;
            lastValue = 0;
            settleUntil = DateTime.UtcNow.AddSeconds(2);
            buffer.SetText("", 0);
            shown.Clear();
            shownLines.Clear();
            Append(vm.VisibleEntries.ToList());
            ScrollToEnd();
        }
        // Changes that only dropped entries from the top and added some at the end (a followed log
        // trims one RemoveAt(0) per line it adds): delete the dropped lines and append the new ones
        // instead of re-rendering ~2000 entries.
        bool TrimAndAppend()
        {
            var now = vm.VisibleEntries;
            if (shown.Count == 0 || now.Count == 0) return false;
            var first = shown.FindIndex(e => ReferenceEquals(e, now[0]));
            if (first < 0) return false;
            var kept = shown.Count - first;
            if (kept > now.Count) return false;
            for (var i = 0; i < kept; i++)
            {
                if (!ReferenceEquals(shown[first + i], now[i])) return false;
            }
            if (first > 0)
            {
                var lines = shownLines.Take(first).Sum();
                buffer.GetStartIter(out var start);
                buffer.GetIterAtLine(out var cut, lines);
                buffer.Delete(start, cut);
                shown.RemoveRange(0, first);
                shownLines.RemoveRange(0, first);
            }
            Append(now.Skip(kept).ToList());
            return true;
        }
        // The changes of one main-loop iteration (a poll adds and trims entry by entry) become one
        // buffer edit after it: whatever happened, the buffer is brought to VisibleEntries by
        // trimming its top and appending, or re-rendered when that does not fit (file, filter).
        var context = SynchronizationContext.Current
            ?? throw new InvalidOperationException("create the Logs page on the GTK main thread");
        var syncQueued = false;
        void Sync()
        {
            syncQueued = false;
            if (TrimAndAppend()) Followed();
            else Rebuild();
        }
        vm.VisibleEntries.CollectionChanged += (_, _) =>
        {
            if (syncQueued) return;
            syncQueued = true;
            context.Post(_ => Sync(), null);
        };
        // The severity colours follow light / dark.
        Adw.StyleManager.GetDefault().OnNotify += (_, args) =>
        {
            if (args.Pspec.GetName() == "dark") Rebuild();
        };
        Rebuild();

        string[] fileLabels = [];
        vm.Bind(() =>
        {
            updatingFiles = true;
            // A new model only when the files change: replacing it while the picker is delivering
            // a choice (SelectedFile set from its "selected" notify) frees its rows and crashes.
            var labels = vm.Files.Select(file => file.Label).ToArray();
            if (!labels.SequenceEqual(fileLabels))
            {
                fileLabels = labels;
                files.SetModel(Gtk.StringList.New(labels));
            }
            var index = vm.Files.ToList().IndexOf(vm.SelectedFile);
            var position = index < 0 ? Gtk.Constants.INVALID_LIST_POSITION : (uint)index;
            if (files.GetSelected() != position) files.SetSelected(position);
            updatingFiles = false;
        }, nameof(vm.Files), nameof(vm.SelectedFile));
        vm.Bind(() =>
        {
            updatingSeverity = true;
            var index = (uint)Math.Max(0, vm.SeverityOptions.ToList().IndexOf(vm.SelectedSeverity));
            if (severity.GetSelected() != index) severity.SetSelected(index);
            updatingSeverity = false;
        }, nameof(vm.SelectedSeverity));
        vm.Bind(() =>
        {
            if (search.GetText() != vm.SearchText) search.SetText(vm.SearchText);
        }, nameof(vm.SearchText));
        vm.Bind(() =>
        {
            following.SetVisible(vm.IsFollowing && !vm.IsMissing);
            stack.SetVisibleChildName(vm.IsMissing ? "missing" : vm.HasNoMatches ? "empty" : "log");
            missing.SetTitle(vm.SelectedFile.Label);
            path.SetText(vm.DisplayPath);
        }, nameof(vm.IsFollowing), nameof(vm.IsMissing), nameof(vm.SelectedFile), nameof(vm.HasNoMatches));

        // Tail only while the page is on screen.
        root.OnMap += (_, _) => vm.Start();
        root.OnUnmap += (_, _) => vm.Stop();
        return root;
    }

    /// <summary>One entry as Pango markup: time, severity, source, message, fields.</summary>
    private static string Markup(LogEntry entry)
    {
        var (warning, danger, dim) = Theme.MarkupColors;
        var line = new StringBuilder();
        if (entry.TimeText.Length > 0)
            line.Append($"<span font_family=\"monospace\" foreground=\"{dim}\">").Append(Escape(entry.TimeText)).Append("</span>  ");
        if (entry.SeverityText.Length > 0)
        {
            var color = entry.Tone switch
            {
                StatusTone.Bad => $" foreground=\"{danger}\"",
                StatusTone.Caution => $" foreground=\"{warning}\"",
                _ => entry.IsDim ? $" foreground=\"{dim}\"" : "",
            };
            // Padded to the longest level so the messages line up.
            line.Append($"<span font_family=\"monospace\" weight=\"bold\"{color}>")
                .Append(entry.SeverityText.PadRight(5)).Append("</span>  ");
        }
        if (!string.IsNullOrEmpty(entry.Source))
            line.Append($"<span size=\"smaller\" foreground=\"{dim}\">").Append(Escape(entry.Source)).Append("</span>  ");
        // Debug / trace messages read dimmed too.
        line.Append(entry.IsDim ? $"<span foreground=\"{dim}\">{Escape(entry.DisplayMessage)}</span>" : Escape(entry.DisplayMessage));
        if (entry.HasFields)
            line.Append($"  <span foreground=\"{dim}\">").Append(Escape(entry.FieldsText)).Append("</span>");
        return line.ToString();
    }

    private static string Escape(string text) =>
        text.Replace("&", "&amp;").Replace("<", "&lt;").Replace(">", "&gt;").Replace("\"", "&quot;");
}
