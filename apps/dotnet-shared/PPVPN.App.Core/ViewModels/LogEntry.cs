using System.Globalization;
using System.Text;
using System.Text.RegularExpressions;

namespace PPVPN.App.Core.ViewModels;

/// <summary>Severity of a log line, lowest first. <see cref="Unknown"/>: a line no format matched.</summary>
public enum LogSeverity { Unknown, Trace, Debug, Info, Warn, Error }

/// <summary>A <c>key=value</c> of a log line, the value unquoted.</summary>
public sealed record LogField(string Key, string Value);

/// <summary>
/// One parsed log line (plus the lines that continue it, e.g. a panic message spanning lines).
/// <see cref="Raw"/> is the original text; <see cref="Message"/> is <see cref="Raw"/> when no
/// format matched.
/// </summary>
public sealed record LogEntry(
    DateTimeOffset? Time,
    LogSeverity Severity,
    string? Source,
    string Message,
    IReadOnlyList<LogField> Fields,
    string Raw,
    string TimeText)
{
    /// <summary>"INFO", "WARN", …; empty for an unparsed line (log levels read the same in every language).</summary>
    public string SeverityText => Severity switch
    {
        LogSeverity.Trace => "TRACE",
        LogSeverity.Debug => "DEBUG",
        LogSeverity.Info => "INFO",
        LogSeverity.Warn => "WARN",
        LogSeverity.Error => "ERROR",
        _ => "",
    };

    /// <summary>Colour of the severity badge / line: error red, warning amber, the rest neutral.</summary>
    public StatusTone Tone => Severity switch
    {
        LogSeverity.Error => StatusTone.Bad,
        LogSeverity.Warn => StatusTone.Caution,
        _ => StatusTone.Neutral,
    };

    /// <summary>Debug / trace lines read dimmed.</summary>
    public bool IsDim => Severity is LogSeverity.Debug or LogSeverity.Trace;

    public bool HasFields => Fields.Count > 0;

    /// <summary>Characters of a message shown; <see cref="Raw"/> keeps the rest for copying.</summary>
    public const int DisplayLimit = 4000;

    /// <summary>
    /// <see cref="Message"/> cut to <see cref="DisplayLimit"/> characters: one endless line (a dumped
    /// response, a long backtrace) would otherwise stall the list's text layout.
    /// </summary>
    public string DisplayMessage => Message.Length <= DisplayLimit ? Message : Message[..DisplayLimit] + " …";

    /// <summary>
    /// What a screen reader announces for a row (list items fall back to ToString): time, level,
    /// message and fields, not the record's member dump.
    /// </summary>
    public override string ToString() =>
        string.Join(" ", new[] { TimeText, SeverityText, Message, FieldsText }.Where(part => part.Length > 0));

    /// <summary>The fields as one line, <c>key=value</c> separated by spaces (values quoted when they contain spaces).</summary>
    public string FieldsText => string.Join(" ", Fields.Select(f => $"{f.Key}={LogParser.Quote(f.Value)}"));
}

/// <summary>
/// Parses the lines of the log files the Logs page shows. Formats:
/// <list type="bullet">
/// <item>client (Rust <c>tracing</c> fmt): <c>2026-09-30T16:42:45.634276Z  INFO ppvpn_client::enhanced: message key=value key="quoted"</c>
/// — RFC 3339 UTC time, level right-aligned to 5, optional spans <c>name{fields}:</c>, the target, then
/// the message with its fields appended as trailing <c>key=value</c> tokens.</item>
/// <item>core (logfmt): <c>2026-09-30T18:27:12.661737Z level=info msg="apply timing" outcome=ok total_ms=1</c>
/// (a leading <c>time=</c> key is accepted too).</item>
/// <item>service (log4rs): <c>[2026-09-30 23:04:45.362][INFO] message</c>, local time.</item>
/// </list>
/// Anything else continues the previous entry (see <see cref="Append"/>) or stands as an
/// <see cref="LogSeverity.Unknown"/> entry.
/// </summary>
public static class LogParser
{
    static readonly Regex Tracing = new(
        @"^(?<time>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2}))\s+(?<level>TRACE|DEBUG|INFO|WARN|ERROR)\s(?<rest>.*)$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant);

    /// <summary>Spans (<c>name{…}:</c>, any number) then the target (<c>crate::module</c>) and ": ".</summary>
    static readonly Regex TracingTarget = new(
        @"^\s*(?<spans>(?:[A-Za-z_][\w:]*(?:\{[^}]*\})?:\s?)*?)(?<target>[A-Za-z_]\w*(?:::\w+)*): (?<msg>.*)$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant);

    static readonly Regex LogfmtStart = new(
        @"^(?:(?<time>\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2}))\s+)?(?<rest>(?:time|level)=.*)$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant);

    static readonly Regex Log4rs = new(
        @"^\[(?<time>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?)\]\[(?<level>[A-Za-z]+)\]\s?(?<msg>.*)$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant);

    /// <summary>A trailing <c> key=value</c> (value quoted or a run without spaces) of a tracing message.</summary>
    static readonly Regex TrailingField = new(
        @"\s(?<key>[A-Za-z_][\w.]*)=(?<value>""(?:[^""\\]|\\.)*""|[^\s""]+)$",
        RegexOptions.Compiled | RegexOptions.CultureInvariant);

    /// <summary>
    /// <paramref name="line"/> as an entry of its own, or null when it matches no format (it then
    /// continues the previous entry). Times are shown in <paramref name="zone"/>.
    /// </summary>
    public static LogEntry? TryParse(string line, TimeZoneInfo zone)
    {
        if (Tracing.Match(line) is { Success: true } tracing)
        {
            var time = ParseUtc(tracing.Groups["time"].Value);
            var severity = Severity(tracing.Groups["level"].Value);
            var rest = tracing.Groups["rest"].Value;
            string? source = null;
            var message = rest.TrimStart();
            if (TracingTarget.Match(rest) is { Success: true } target)
            {
                source = target.Groups["target"].Value;
                message = target.Groups["msg"].Value;
            }
            var (text, fields) = SplitTrailingFields(message);
            return new LogEntry(time, severity, source, text, fields, line, TimeText(time, zone));
        }
        if (LogfmtStart.Match(line) is { Success: true } logfmt)
        {
            var pairs = ParseLogfmt(logfmt.Groups["rest"].Value);
            DateTimeOffset? time = logfmt.Groups["time"].Success ? ParseUtc(logfmt.Groups["time"].Value) : null;
            var severity = LogSeverity.Unknown;
            string message = "";
            var fields = new List<LogField>();
            foreach (var (key, value) in pairs)
            {
                switch (key)
                {
                    case "time" when time is null:
                        time = ParseUtc(value);
                        break;
                    case "level":
                        severity = Severity(value);
                        break;
                    case "msg" or "message" when message.Length == 0:
                        message = value;
                        break;
                    default:
                        fields.Add(new LogField(key, value));
                        break;
                }
            }
            if (severity == LogSeverity.Unknown && time is null) return null;
            return new LogEntry(time, severity, null, message, fields, line, TimeText(time, zone));
        }
        if (Log4rs.Match(line) is { Success: true } log4rs)
        {
            DateTimeOffset? time = DateTime.TryParse(log4rs.Groups["time"].Value, CultureInfo.InvariantCulture,
                DateTimeStyles.None, out var local)
                ? new DateTimeOffset(local, zone.GetUtcOffset(local))
                : null;
            return new LogEntry(time, Severity(log4rs.Groups["level"].Value), null, log4rs.Groups["msg"].Value, [],
                line, TimeText(time, zone));
        }
        return null;
    }

    /// <summary>
    /// Adds <paramref name="line"/> to <paramref name="entries"/>: a new entry, or appended to the
    /// last one when it matches no format (a line of a multi-line message). Returns true when the
    /// last entry was replaced rather than a new one added.
    /// </summary>
    public static bool Append(IList<LogEntry> entries, string line, TimeZoneInfo zone)
    {
        if (TryParse(line, zone) is { } entry)
        {
            entries.Add(entry);
            return false;
        }
        if (entries.Count > 0)
        {
            var index = entries.Count - 1;
            var last = entries[index];
            entries[index] = last with { Message = last.Message + "\n" + line, Raw = last.Raw + "\n" + line };
            return true;
        }
        entries.Add(new LogEntry(null, LogSeverity.Unknown, null, line, [], line, ""));
        return false;
    }

    /// <summary>The message without its trailing <c>key=value</c> fields, and the fields in order.</summary>
    internal static (string Message, IReadOnlyList<LogField> Fields) SplitTrailingFields(string message)
    {
        var fields = new List<LogField>();
        // The leading space lets a message made only of fields (tracing without one) match too.
        var text = " " + message;
        while (TrailingField.Match(text) is { Success: true } match)
        {
            fields.Insert(0, new LogField(match.Groups["key"].Value, Unquote(match.Groups["value"].Value)));
            text = text[..match.Index];
        }
        return (text.Trim(), fields);
    }

    /// <summary>logfmt pairs in order; a bare word becomes a key with an empty value.</summary>
    internal static List<(string Key, string Value)> ParseLogfmt(string text)
    {
        var pairs = new List<(string, string)>();
        var i = 0;
        while (i < text.Length)
        {
            while (i < text.Length && text[i] == ' ') i++;
            if (i >= text.Length) break;
            var keyStart = i;
            while (i < text.Length && text[i] != '=' && text[i] != ' ') i++;
            var key = text[keyStart..i];
            if (i >= text.Length || text[i] == ' ')
            {
                pairs.Add((key, ""));
                continue;
            }
            i++; // '='
            string value;
            if (i < text.Length && text[i] == '"')
            {
                var start = i;
                i++;
                while (i < text.Length && text[i] != '"')
                {
                    if (text[i] == '\\') i++;
                    i++;
                }
                i = Math.Min(i + 1, text.Length);
                value = Unquote(text[start..i]);
            }
            else
            {
                var start = i;
                while (i < text.Length && text[i] != ' ') i++;
                value = text[start..i];
            }
            pairs.Add((key, value));
        }
        return pairs;
    }

    /// <summary><c>"a \"b\""</c> → <c>a "b"</c> (Go / Rust Debug escapes); unquoted text unchanged.</summary>
    internal static string Unquote(string value)
    {
        if (value.Length < 2 || value[0] != '"' || value[^1] != '"') return value;
        var inner = value[1..^1];
        if (!inner.Contains('\\')) return inner;
        var result = new StringBuilder(inner.Length);
        for (var i = 0; i < inner.Length; i++)
        {
            if (inner[i] != '\\' || i + 1 == inner.Length)
            {
                result.Append(inner[i]);
                continue;
            }
            var next = inner[++i];
            result.Append(next switch
            {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                _ => next,
            });
        }
        return result.ToString();
    }

    /// <summary>A value as logfmt writes it: quoted (with escapes) when empty or containing spaces, quotes or '='.</summary>
    internal static string Quote(string value) =>
        value.Length > 0 && value.IndexOfAny([' ', '"', '=', '\n', '\t']) < 0
            ? value
            : "\"" + value.Replace("\\", "\\\\").Replace("\"", "\\\"").Replace("\n", "\\n").Replace("\t", "\\t") + "\"";

    static LogSeverity Severity(string level) => level.Trim().ToUpperInvariant() switch
    {
        "TRACE" => LogSeverity.Trace,
        "DEBUG" => LogSeverity.Debug,
        "INFO" => LogSeverity.Info,
        "WARN" or "WARNING" => LogSeverity.Warn,
        "ERROR" or "ERR" or "FATAL" or "CRITICAL" or "PANIC" => LogSeverity.Error,
        _ => LogSeverity.Unknown,
    };

    static DateTimeOffset? ParseUtc(string text) =>
        DateTimeOffset.TryParse(text, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal, out var time)
            ? time
            : null;

    /// <summary>"HH:mm:ss.fff" in <paramref name="zone"/>; empty without a time.</summary>
    static string TimeText(DateTimeOffset? time, TimeZoneInfo zone) =>
        time is { } value
            ? TimeZoneInfo.ConvertTime(value, zone).ToString("HH:mm:ss.fff", CultureInfo.InvariantCulture)
            : "";
}
