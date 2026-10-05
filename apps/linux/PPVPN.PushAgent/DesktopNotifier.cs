using System.Collections.Concurrent;
using System.Diagnostics;
using System.Globalization;
using PPVPN.Ffi;
using Tmds.DBus.Protocol;

namespace PPVPN.PushAgent;

/// <summary>
/// org.freedesktop.Notifications client. Notifications carry the desktop-entry hint, so the
/// shell shows them under the app's name and icon; a click (the default action) runs
/// <c>ppvpn --open-notification &lt;push id&gt;</c>, which the running app receives through its
/// command line and resolves (link, inbox message or the shown push) itself.
/// </summary>
internal sealed class DesktopNotifier : IDisposable
{
    private const string ServerName = "org.freedesktop.Notifications";
    private const string ServerPath = "/org/freedesktop/Notifications";
    private const string DesktopEntry = "com.peakpassvpn.ppvpn.desktop";
    private static readonly TimeSpan CallTimeout = TimeSpan.FromSeconds(10);

    /// <summary>Notifications still on screen (or in the shell's list) that a click can open.</summary>
    private const int MaxTracked = 200;

    private readonly string _appPath;
    private readonly DBusConnection? _connection;
    private readonly ConcurrentDictionary<uint, PushMessage> _shown = new();
    private readonly ConcurrentDictionary<uint, string> _activationTokens = new();
    private readonly List<IDisposable> _subscriptions = [];
    private bool? _bodyMarkup;
    private volatile bool _disposed;

    public DesktopNotifier(string appPath)
    {
        _appPath = appPath;
        if (DBusAddress.Session is { } address) _connection = new DBusConnection(address);
    }

    /// <summary>A notification server started (or restarted).</summary>
    public event Action? ServerAppeared;

    /// <summary>The session bus connection was lost.</summary>
    public event Action? Disconnected;

    public async Task<bool> ConnectAsync()
    {
        if (_connection is null) return false;
        try
        {
            await _connection.ConnectAsync();
        }
        catch (Exception error)
        {
            Console.Error.WriteLine($"ppvpn-push-agent: {error.Message}");
            return false;
        }
        _subscriptions.Add(await _connection.WatchSignalAsync(
            null, "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged",
            static (message, _) =>
            {
                var reader = message.GetBodyReader();
                var name = reader.ReadString();
                reader.ReadString(); // old owner
                return (Name: name, HasOwner: reader.ReadString().Length > 0);
            },
            (Notification<(string Name, bool HasOwner)> notification) =>
            {
                // Signal handlers must not throw: Tmds disconnects on an exception.
                if (!notification.HasValue || notification.Value.Name != ServerName || !notification.Value.HasOwner) return;
                Console.Error.WriteLine("ppvpn-push-agent: notification server appeared");
                _bodyMarkup = null; // another server, other capabilities
                ServerAppeared?.Invoke();
            },
            ObserverFlags.None, emitOnCapturedContext: false));
        _ = RaiseDisconnectedAsync(_connection);
        // Sent just before ActionInvoked by servers that support XDG activation (Wayland focus).
        _subscriptions.Add(await _connection.WatchSignalAsync(
            null, ServerPath, ServerName, "ActivationToken",
            static (message, _) =>
            {
                var reader = message.GetBodyReader();
                return (Id: reader.ReadUInt32(), Token: reader.ReadString());
            },
            (Notification<(uint Id, string Token)> notification) =>
            {
                if (notification.HasValue && _shown.ContainsKey(notification.Value.Id))
                    _activationTokens[notification.Value.Id] = notification.Value.Token;
            },
            ObserverFlags.None, emitOnCapturedContext: false));
        _subscriptions.Add(await _connection.WatchSignalAsync(
            null, ServerPath, ServerName, "ActionInvoked",
            static (message, _) =>
            {
                var reader = message.GetBodyReader();
                return (Id: reader.ReadUInt32(), Action: reader.ReadString());
            },
            (Notification<(uint Id, string Action)> notification) =>
            {
                if (notification.HasValue && notification.Value.Action == "default") Open(notification.Value.Id);
            },
            ObserverFlags.None, emitOnCapturedContext: false));
        _subscriptions.Add(await _connection.WatchSignalAsync(
            null, ServerPath, ServerName, "NotificationClosed",
            static (message, _) => message.GetBodyReader().ReadUInt32(),
            (Notification<uint> notification) =>
            {
                if (!notification.HasValue) return;
                _shown.TryRemove(notification.Value, out _);
                _activationTokens.TryRemove(notification.Value, out _);
            },
            ObserverFlags.None, emitOnCapturedContext: false));
        return true;
    }

    /// <summary>
    /// Hands <paramref name="message"/> to the notification server; false when there is none
    /// (the agent retries on <see cref="ServerAppeared"/> or later). Blocks the calling thread.
    /// </summary>
    public bool Show(PushMessage message)
    {
        if (_connection is null) return false;
        try
        {
            var markup = _bodyMarkup ??= HasBodyMarkup();
            var body = markup ? EscapeMarkup(message.Body) : message.Body;
            var id = _connection.CallMethodAsync(
                CreateMessage((ref MessageWriter writer) =>
                {
                    writer.WriteMethodCallHeader(ServerName, ServerPath, ServerName, "Notify", "susssasa{sv}i", MessageFlags.None);
                    writer.WriteString("PPVPN");
                    writer.WriteUInt32(0);
                    writer.WriteString(DesktopEntry);
                    writer.WriteString(message.Title);
                    writer.WriteString(body);
                    writer.WriteArray(new[] { "default", OpenLabel() });
                    var hints = writer.WriteDictionaryStart();
                    writer.WriteDictionaryEntryStart();
                    writer.WriteString("desktop-entry");
                    writer.WriteSignature("s");
                    writer.WriteString(DesktopEntry);
                    writer.WriteDictionaryEntryStart();
                    writer.WriteString("urgency");
                    writer.WriteSignature("y");
                    writer.WriteByte(message.Severity == MessageSeverity.Critical ? (byte)2 : (byte)1);
                    writer.WriteDictionaryEnd(hints);
                    writer.WriteInt32(-1);
                }),
                static (reply, _) => reply.GetBodyReader().ReadUInt32()).WaitAsync(CallTimeout).GetAwaiter().GetResult();
            _shown[id] = message;
            Trim();
            return true;
        }
        catch (Exception error)
        {
            // No notification server (ServiceUnknown), or the bus is gone.
            Console.Error.WriteLine($"ppvpn-push-agent: notify failed: {error.Message}");
            return false;
        }
    }

    private async Task RaiseDisconnectedAsync(DBusConnection connection)
    {
        var reason = await connection.DisconnectedAsync();
        if (_disposed) return;
        Console.Error.WriteLine($"ppvpn-push-agent: session bus lost: {reason?.Message}");
        Disconnected?.Invoke();
    }

    public void Dispose()
    {
        _disposed = true;
        foreach (var subscription in _subscriptions) subscription.Dispose();
        _connection?.Dispose();
    }

    private void Open(uint notificationId)
    {
        if (!_shown.TryRemove(notificationId, out var message)) return;
        _activationTokens.TryRemove(notificationId, out var token);
        var start = new ProcessStartInfo(_appPath) { UseShellExecute = false };
        start.ArgumentList.Add("--open-notification");
        start.ArgumentList.Add(message.Id.ToString(CultureInfo.InvariantCulture));
        if (!string.IsNullOrEmpty(token))
        {
            start.Environment["XDG_ACTIVATION_TOKEN"] = token;
            start.Environment["DESKTOP_STARTUP_ID"] = token;
        }
        try
        {
            using var process = Process.Start(start);
        }
        catch (Exception error)
        {
            Console.Error.WriteLine($"ppvpn-push-agent: could not start {_appPath}: {error.Message}");
        }
    }

    /// <summary>Servers that never report closing (some keep a history) must not grow the map.</summary>
    private void Trim()
    {
        if (_shown.Count <= MaxTracked) return;
        foreach (var id in _shown.Keys.Order().Take(_shown.Count - MaxTracked))
        {
            _shown.TryRemove(id, out _);
            _activationTokens.TryRemove(id, out _);
        }
    }

    private bool HasBodyMarkup()
    {
        var capabilities = _connection!.CallMethodAsync(
            CreateMessage((ref MessageWriter writer) =>
                writer.WriteMethodCallHeader(ServerName, ServerPath, ServerName, "GetCapabilities", null, MessageFlags.None)),
            static (reply, _) => reply.GetBodyReader().ReadArrayOfString()).WaitAsync(CallTimeout).GetAwaiter().GetResult();
        return capabilities.Contains("body-markup");
    }

    /// <summary>With body-markup the server parses a subset of HTML: keep text literal.</summary>
    private static string EscapeMarkup(string text) =>
        text.Replace("&", "&amp;").Replace("<", "&lt;").Replace(">", "&gt;");

    private static string OpenLabel()
    {
        var language = Environment.GetEnvironmentVariable("LANGUAGE")?.Split(':')[0]
            ?? Environment.GetEnvironmentVariable("LC_ALL")
            ?? Environment.GetEnvironmentVariable("LC_MESSAGES")
            ?? Environment.GetEnvironmentVariable("LANG");
        return language?.StartsWith("zh", StringComparison.Ordinal) == true ? "打开" : "Open";
    }

    // MessageWriter is a ref struct, so bodies are written through a callback.
    private delegate void WriteBody(ref MessageWriter writer);

    private MessageBuffer CreateMessage(WriteBody body)
    {
        var writer = _connection!.GetMessageWriter();
        try
        {
            body(ref writer);
            return writer.CreateMessage();
        }
        finally
        {
            writer.Dispose();
        }
    }
}
