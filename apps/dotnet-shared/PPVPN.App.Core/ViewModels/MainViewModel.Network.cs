using PPVPN.App.Core.Backend;

namespace PPVPN.App.Core.ViewModels;

public sealed partial class MainViewModel
{
    /// <summary>How long network events must settle before the client hears of them (a Wi-Fi switch fires several).</summary>
    internal static readonly TimeSpan NetworkChangeDebounce = TimeSpan.FromSeconds(1);

    readonly object _networkGate = new();
    ITimer? _networkTimer;
    bool _networkStopped;

    /// <summary>
    /// The OS reported a network change (an interface, an address or the default route). Platforms
    /// call it on every event, from any thread; once events pause for <see cref="NetworkChangeDebounce"/>
    /// the client is told (<see cref="IClientBackend.NetworkChanged"/>). Our own TUN interface coming
    /// or going counts too; an extra early check is harmless.
    /// </summary>
    public void OnNetworkChanged()
    {
        lock (_networkGate)
        {
            if (_networkStopped) return;
            _networkTimer ??= _time.CreateTimer(_ => NotifyNetworkChanged(), null, Timeout.InfiniteTimeSpan, Timeout.InfiniteTimeSpan);
            _networkTimer.Change(NetworkChangeDebounce, Timeout.InfiniteTimeSpan);
        }
    }

    void NotifyNetworkChanged()
    {
        lock (_networkGate)
        {
            if (_networkStopped) return;
        }
        try
        {
            _log.Info("network changed: client told to check its data path");
            Backend.NetworkChanged();
        }
        catch (Exception e)
        {
            _log.Warn($"network change: {e.Message}");
        }
    }

    void StopNetworkWatch()
    {
        lock (_networkGate)
        {
            _networkStopped = true;
            _networkTimer?.Dispose();
            _networkTimer = null;
        }
    }
}
