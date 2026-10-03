using PPVPN.App.Core.ViewModels;

namespace PPVPN.App.Core.Tests;

public sealed class LogParserTests
{
    static readonly TimeZoneInfo Beijing = ManualTime.Beijing;

    [Fact]
    public void ClientTracingLinesSplitTimeLevelTargetMessageAndFields()
    {
        var entry = LogParser.TryParse(
            "2026-09-30T18:49:02.356264Z  INFO ppvpn_client::enhanced: service watch ended generation=43 end=Stopping",
            Beijing)!;
        Assert.Equal(new DateTimeOffset(2026, 9, 30, 18, 49, 2, TimeSpan.Zero).AddTicks(3562640), entry.Time);
        Assert.Equal("02:49:02.356", entry.TimeText); // UTC+8, the next day
        Assert.Equal(LogSeverity.Info, entry.Severity);
        Assert.Equal("INFO", entry.SeverityText);
        Assert.Equal("ppvpn_client::enhanced", entry.Source);
        Assert.Equal("service watch ended", entry.Message);
        Assert.Equal([new LogField("generation", "43"), new LogField("end", "Stopping")], entry.Fields);
        Assert.Equal("generation=43 end=Stopping", entry.FieldsText);

        // Quoted (Debug) values are unquoted; brackets and parentheses in a message stay message.
        var started = LogParser.TryParse(
            "2026-09-30T18:46:06.317995Z  INFO ppvpn_client::logging: ppvpn-client started version=\"0.1.0\"", Beijing)!;
        Assert.Equal("ppvpn-client started", started.Message);
        Assert.Equal([new LogField("version", "0.1.0")], started.Fields);
        var warn = LogParser.TryParse(
            "2026-09-30T16:40:53.330622Z  WARN ppvpn_client::enhanced: preflight: another tunnel owns the network (competitors [\"Surge\"], route Some(\"utun5\")); not starting TUN",
            Beijing)!;
        Assert.Equal(LogSeverity.Warn, warn.Severity);
        Assert.Equal(StatusTone.Caution, warn.Tone);
        Assert.Equal("preflight: another tunnel owns the network (competitors [\"Surge\"], route Some(\"utun5\")); not starting TUN", warn.Message);
        Assert.Empty(warn.Fields);

        // Spans before the target; escapes inside a quoted value; a message of fields only.
        var spans = LogParser.TryParse(
            "2026-10-01T01:00:00.000000Z DEBUG connect{generation=3}: ppvpn_client::x: hello x=\"a \\\"b\\\"\"", Beijing)!;
        Assert.Equal(LogSeverity.Debug, spans.Severity);
        Assert.True(spans.IsDim);
        Assert.Equal("ppvpn_client::x", spans.Source);
        Assert.Equal("hello", spans.Message);
        Assert.Equal([new LogField("x", "a \"b\"")], spans.Fields);
        var onlyFields = LogParser.TryParse("2026-10-01T01:00:00Z ERROR ppvpn_client::x: a=1 b=2", Beijing)!;
        Assert.Equal("", onlyFields.Message);
        Assert.Equal(2, onlyFields.Fields.Count);
        Assert.Equal(StatusTone.Bad, onlyFields.Tone);
    }

    [Fact]
    public void CoreLogfmtLinesTakeLevelAndMsgOutOfTheFields()
    {
        var entry = LogParser.TryParse(
            "2026-09-30T09:22:47.12576Z level=info msg=\"serve starting\" core_version=0.4.4 state_dir=\"/Volumes/x/Library/Application Support/PPVPN\" note=\"tab\\there\"",
            Beijing)!;
        Assert.Equal(LogSeverity.Info, entry.Severity);
        Assert.Equal("17:22:47.125", entry.TimeText);
        Assert.Null(entry.Source);
        Assert.Equal("serve starting", entry.Message);
        Assert.Equal(
            [
                new LogField("core_version", "0.4.4"),
                new LogField("state_dir", "/Volumes/x/Library/Application Support/PPVPN"),
                new LogField("note", "tab\there"),
            ],
            entry.Fields);
        Assert.Equal("core_version=0.4.4 state_dir=\"/Volumes/x/Library/Application Support/PPVPN\" note=\"tab\\there\"", entry.FieldsText);

        // A leading time= key, "warning", a bare word.
        var keyed = LogParser.TryParse("time=2026-09-30T09:22:47Z level=warning msg=slow flag", Beijing)!;
        Assert.Equal(LogSeverity.Warn, keyed.Severity);
        Assert.Equal("17:22:47.000", keyed.TimeText);
        Assert.Equal("slow", keyed.Message);
        Assert.Equal([new LogField("flag", "")], keyed.Fields);
    }

    [Fact]
    public void ServiceLinesAreLocalTime()
    {
        var entry = LogParser.TryParse("[2026-09-30 23:04:45.362][ERROR] ppvpn-core startup timed out", Beijing)!;
        Assert.Equal(LogSeverity.Error, entry.Severity);
        Assert.Equal("23:04:45.362", entry.TimeText);
        Assert.Equal("ppvpn-core startup timed out", entry.Message);
    }

    [Fact]
    public void UnknownLinesContinueThePreviousEntryOrStandAlone()
    {
        var entries = new List<LogEntry>();
        Assert.False(LogParser.Append(entries, "   at frame 0", Beijing)); // nothing to continue
        Assert.Equal(LogSeverity.Unknown, entries[0].Severity);
        Assert.Equal("   at frame 0", entries[0].Message);
        Assert.Equal("", entries[0].TimeText);

        Assert.False(LogParser.Append(entries, "2026-09-30T16:05:46Z ERROR ppvpn_client::logging: panic: boom", Beijing));
        Assert.True(LogParser.Append(entries, "stack backtrace:", Beijing));
        Assert.True(LogParser.Append(entries, "  0: rust_begin_unwind", Beijing));
        Assert.Equal(2, entries.Count);
        Assert.Equal("panic: boom\nstack backtrace:\n  0: rust_begin_unwind", entries[1].Message);
        Assert.Equal(
            "2026-09-30T16:05:46Z ERROR ppvpn_client::logging: panic: boom\nstack backtrace:\n  0: rust_begin_unwind",
            entries[1].Raw);
    }
}
