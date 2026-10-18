using Tmds.DBus.Protocol;

namespace PPVPN.Linux.Platform;

/// <summary>
/// The system's suspend and resume, from logind's <c>PrepareForSleep</c> signal on the system
/// bus: <c>true</c> before it sleeps, <c>false</c> after it woke. Without a delay inhibitor the
/// sleep may come before the cores heard of it; the engine then goes by its own detection, and a
/// wake without a sleep before it changes nothing (host-integration 4.7).
/// </summary>
public sealed class SystemSleep : IDisposable
{
    private readonly DBusConnection _connection;
    private IDisposable? _subscription;

    private SystemSleep(DBusConnection connection) => _connection = connection;

    /// <summary>Calls <paramref name="changed"/> (on a bus thread) at each sleep and wake; null without a system bus or logind.</summary>
    public static async Task<SystemSleep?> StartAsync(Action<bool> changed, AppLog log)
    {
        if (DBusAddress.System is null) return null;
        var sleep = new SystemSleep(new DBusConnection(DBusAddress.System));
        try
        {
            await sleep._connection.ConnectAsync();
            sleep._subscription = await sleep._connection.WatchSignalAsync(
                "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "PrepareForSleep",
                static (message, _) => message.GetBodyReader().ReadBool(),
                (Notification<bool> notification) =>
                {
                    if (notification.HasValue) changed(notification.Value);
                },
                ObserverFlags.None, emitOnCapturedContext: false);
            return sleep;
        }
        catch (Exception error)
        {
            log.Warn($"system sleep not followed: {error.Message}");
            sleep.Dispose();
            return null;
        }
    }

    public void Dispose()
    {
        _subscription?.Dispose();
        _connection.Dispose();
    }
}
