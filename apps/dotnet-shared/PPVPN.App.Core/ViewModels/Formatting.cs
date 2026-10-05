using System.Globalization;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>Visual meaning of a value (latency, message severity); views map it to a colour.</summary>
public enum StatusTone { Neutral, Busy, Good, Caution, Bad }

/// <summary>
/// Tone of the connection (design: idle / busy / ok / warn / err): status circle and icon, title
/// colour, title-bar dot, tray icon.
/// </summary>
public enum ConnectionTone { Idle, Busy, Ok, Warn, Error }

/// <summary>What a latency cell shows.</summary>
public enum LatencyKind
{
    /// <summary>Not tested: "—".</summary>
    None,
    /// <summary>Test running: a spinner only.</summary>
    Testing,
    /// <summary>"38 ms", coloured by <see cref="Formatting.LatencyTone"/>.</summary>
    Value,
    /// <summary><c>timeout</c>, secondary colour + clock icon.</summary>
    Timeout,
    /// <summary><c>failed</c>, danger colour + error icon.</summary>
    Failed,
}

public static class Formatting
{
    static readonly string[] Months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

    /// <summary>
    /// A <see cref="TrafficSample"/> rate. <c>UpBps</c>/<c>DownBps</c> are BYTES per second despite
    /// the suffix. As the design: "8.6 MB/s" from 1 MiB/s up, else whole "KB/s" ("0 KB/s" floor).
    /// </summary>
    public static string Rate(ulong bytesPerSecond) =>
        bytesPerSecond >= 1_048_576
            ? (bytesPerSecond / 1_048_576.0).ToString("0.0", CultureInfo.InvariantCulture) + " MB/s"
            : Math.Round(bytesPerSecond / 1024.0, MidpointRounding.AwayFromZero).ToString("0", CultureInfo.InvariantCulture) + " KB/s";

    /// <summary>RFC 3339 → an instant; null when missing or unparseable.</summary>
    public static DateTimeOffset? ParseRfc3339(string? value) =>
        DateTimeOffset.TryParse(value, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal, out var parsed)
            ? parsed
            : null;

    /// <summary>"9:54" (minutes:seconds, never negative).</summary>
    public static string Countdown(TimeSpan remaining)
    {
        var seconds = Math.Max(0, (int)Math.Ceiling(remaining.TotalSeconds));
        return $"{seconds / 60}:{seconds % 60:00}";
    }

    /// <summary>
    /// Lines in failover order: "HKG-A → HKG-B → SZX-R". A line without a label is named by its
    /// place in that order (<c>routeN</c>: "线路 2"); a single line without one shows nothing.
    /// Endpoint keys are internal and never shown.
    /// </summary>
    public static string Routes(Node node, ILocalizer strings)
    {
        var replicas = node.Replicas.OrderBy(r => r.ReplicaOrdinal).ToList();
        if (replicas.Count == 1) return RouteLabel(replicas[0]) ?? "";
        return string.Join(" → ", replicas.Select((replica, index) =>
            RouteLabel(replica) ?? strings.Format("routeN", ("n", index + 1))));
    }

    /// <summary>A line's label from the backend; null without one.</summary>
    public static string? RouteLabel(Replica replica) => replica.Label is { Length: > 0 } label ? label : null;

    /// <summary>The label of <paramref name="endpointKey"/> among <paramref name="node"/>'s replicas; null when unknown or unlabelled.</summary>
    public static string? RouteLabel(Node? node, string endpointKey) =>
        node?.Replicas.FirstOrDefault(r => r.EndpointKey == endpointKey) is { } replica ? RouteLabel(replica) : null;

    /// <summary>
    /// A line's name for a picker: its label, else <c>routeN</c> by its place in failover order
    /// ("线路 2"); null when <paramref name="endpointKey"/> is not one of the node's lines.
    /// </summary>
    public static string? LineName(Node? node, string endpointKey, ILocalizer strings)
    {
        if (node is null) return null;
        var replicas = node.Replicas.OrderBy(r => r.ReplicaOrdinal).ToList();
        var index = replicas.FindIndex(r => r.EndpointKey == endpointKey);
        return index < 0 ? null : RouteLabel(replicas[index]) ?? strings.Format("routeN", ("n", index + 1));
    }

    /// <summary>First replica (the line tried first).</summary>
    public static Replica? FirstReplica(Node? node) =>
        node?.Replicas.OrderBy(r => r.ReplicaOrdinal).FirstOrDefault();

    /// <summary>Design thresholds: &lt; 100 ms success, 100–199 warning, ≥ 200 danger.</summary>
    public static StatusTone LatencyTone(uint milliseconds) =>
        milliseconds < 100 ? StatusTone.Good : milliseconds < 200 ? StatusTone.Caution : StatusTone.Bad;

    public static string Milliseconds(uint? milliseconds) => milliseconds is { } ms ? $"{ms} ms" : "—";

    /// <summary>Long date: "2026年12月31日" / "Dec 31, 2026".</summary>
    public static string LongDate(DateTimeOffset local, string language) =>
        language == JsonLocalizer.Chinese
            ? $"{local.Year}年{local.Month}月{local.Day}日"
            : $"{Months[local.Month - 1]} {local.Day}, {local.Year}";

    /// <summary>Absolute message time: "2026年9月29日 14:29" / "Sep 29, 2026 14:29".</summary>
    public static string AbsoluteTime(DateTimeOffset when, TimeZoneInfo zone, string language)
    {
        var local = TimeZoneInfo.ConvertTime(when, zone);
        return $"{LongDate(local, language)} {local:HH:mm}";
    }

    /// <summary>
    /// Relative message time (design <c>relTime</c>): <c>justNow</c> under a minute, <c>minAgo</c>
    /// under an hour, <c>timeToday</c> / <c>timeYesterday</c> ("今天 14:20"), then "9月21日" /
    /// "Sep 21", with the year when it is not the current one.
    /// </summary>
    public static string RelativeTime(DateTimeOffset when, DateTimeOffset now, TimeZoneInfo zone, ILocalizer strings)
    {
        var age = now - when;
        if (age < TimeSpan.FromMinutes(1)) return strings.Get("justNow");
        if (age < TimeSpan.FromHours(1)) return strings.Format("minAgo", ("n", (int)age.TotalMinutes));
        var local = TimeZoneInfo.ConvertTime(when, zone);
        var today = TimeZoneInfo.ConvertTime(now, zone).Date;
        var hm = local.ToString("HH:mm", CultureInfo.InvariantCulture);
        if (local.Date == today) return strings.Format("timeToday", ("t", hm));
        if (local.Date == today.AddDays(-1)) return strings.Format("timeYesterday", ("t", hm));
        var sameYear = local.Year == today.Year;
        return strings.Language == JsonLocalizer.Chinese
            ? (sameYear ? $"{local.Month}月{local.Day}日" : $"{local.Year}年{local.Month}月{local.Day}日")
            : (sameYear ? $"{Months[local.Month - 1]} {local.Day}" : $"{Months[local.Month - 1]} {local.Day}, {local.Year}");
    }

    /// <summary>Badge text: "" at 0, the number up to 99, then "99+".</summary>
    public static string Badge(int count) => count switch
    {
        <= 0 => "",
        > 99 => "99+",
        _ => count.ToString(CultureInfo.InvariantCulture),
    };
}

/// <summary>
/// User-facing text for crate errors. Only the code is shown (key <c>Error_{ErrorCode}</c>,
/// the variant name); the detail goes to the log.
/// </summary>
public static class ErrorMessages
{
    /// <summary>The localisation key for a code: <c>Error_</c> + the variant name.</summary>
    public static string Key(ErrorCode code) => $"Error_{code}";

    /// <summary>Every <see cref="ErrorCode"/> key; string catalogs must define all of them.</summary>
    public static IReadOnlyList<string> AllKeys { get; } = Enum.GetValues<ErrorCode>().Select(Key).ToArray();

    /// <summary>Keys for <see cref="ClientException"/> variants other than Failed, and fallbacks.</summary>
    public static IReadOnlyList<string> OtherKeys { get; } =
        ["Error_NotSignedIn", "Error_StandardNotReady", "Error_NotImplemented", "Error_Unexpected", "Error_Unknown"];

    public static string Message(this ILocalizer strings, ErrorCode code)
    {
        var text = strings.Get(Key(code));
        return string.IsNullOrEmpty(text) ? strings.Format("Error_Unknown", ("c", code)) : text;
    }

    public static string Message(this ILocalizer strings, ClientErrorInfo info) => strings.Message(info.Code);

    public static string Message(this ILocalizer strings, Exception error) => error switch
    {
        ClientException.NotSignedIn => strings.Get("Error_NotSignedIn"),
        ClientException.StandardNotReady => strings.Get("Error_StandardNotReady"),
        ClientException.Failed failed => strings.Message(failed.code),
        ClientException.NotImplemented => strings.Get("Error_NotImplemented"),
        _ => strings.Format("Error_Unexpected", ("m", error.Message)),
    };

    public static string Describe(Exception error) => error switch
    {
        ClientException.Failed failed => $"{failed.code}: {failed.detail}",
        _ => $"{error.GetType().Name}: {error.Message}",
    };

    /// <summary>The code of a <see cref="ClientException.Failed"/>, else null.</summary>
    public static ErrorCode? Code(Exception error) => error is ClientException.Failed failed ? failed.code : null;

    /// <summary>
    /// The code a persistent <see cref="ProfileStatus"/> stands for (NoSubscription,
    /// SubscriptionExpired, TeamDisabled, or the Invalid error's code); null for Loading/Ready.
    /// A failure with the current status's code is shown by the content, not a dialog.
    /// </summary>
    public static ErrorCode? Code(ProfileStatus status) => status switch
    {
        ProfileStatus.NoSubscription => ErrorCode.NoSubscription,
        ProfileStatus.SubscriptionExpired => ErrorCode.SubscriptionExpired,
        ProfileStatus.TeamDisabled => ErrorCode.TeamDisabled,
        ProfileStatus.Invalid { Error: var error } => error.Code,
        _ => null,
    };
}
