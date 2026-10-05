using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// The <see cref="ClientListener"/> handed to the backend. The crate calls it
/// from its own (tokio) threads; this captures the UI thread's
/// <see cref="SynchronizationContext"/> at construction and Posts every
/// callback there, preserving order. Portable: WinUI installs a
/// DispatcherQueueSynchronizationContext; the GTK app installs one that posts
/// through the GLib main loop (see README.md). The context must run posted
/// callbacks in FIFO order.
/// </summary>
public sealed class SynchronizedListener : ClientListener
{
    const int TracedCallbacks = 3;

    readonly SynchronizationContext _context;
    readonly int _uiThread;
    readonly Action<ClientSnapshot> _onSnapshot;
    readonly Action<ProbeResult> _onProbe;
    readonly Action<TrafficSample> _onTraffic;
    readonly Action<string>? _trace;
    int _received, _offThread, _misdelivered;

    public SynchronizedListener(
        Action<ClientSnapshot> onSnapshot,
        Action<ProbeResult> onProbe,
        Action<TrafficSample> onTraffic,
        Action<string>? trace = null)
    {
        _context = SynchronizationContext.Current
            ?? throw new InvalidOperationException("Create the listener on the UI thread (no SynchronizationContext).");
        _uiThread = Environment.CurrentManagedThreadId;
        _onSnapshot = onSnapshot;
        _onProbe = onProbe;
        _onTraffic = onTraffic;
        _trace = trace;
    }

    /// <summary>Callbacks received, how many arrived off the UI thread, and how many ran anywhere but the UI thread.</summary>
    public (int Received, int OffThread, int Misdelivered) Stats => (_received, _offThread, _misdelivered);

    public void OnSnapshot(ClientSnapshot snapshot) => Post(nameof(OnSnapshot), () => _onSnapshot(snapshot));

    public void OnProbeResult(ProbeResult result) => Post(nameof(OnProbeResult), () => _onProbe(result));

    public void OnTraffic(TrafficSample sample) => Post(nameof(OnTraffic), () => _onTraffic(sample));

    void Post(string callback, Action deliver)
    {
        var sequence = Interlocked.Increment(ref _received);
        var sourceThread = Environment.CurrentManagedThreadId;
        if (sourceThread != _uiThread) Interlocked.Increment(ref _offThread);

        _context.Post(_ =>
        {
            var thread = Environment.CurrentManagedThreadId;
            if (thread != _uiThread)
            {
                Interlocked.Increment(ref _misdelivered);
                _trace?.Invoke($"listener: {callback} #{sequence} delivered on thread {thread}, expected UI thread {_uiThread}");
            }
            else if (sequence <= TracedCallbacks)
            {
                _trace?.Invoke($"listener: {callback} #{sequence} arrived on thread {sourceThread}, delivered on UI thread {thread}");
            }
            deliver();
        }, null);
    }
}
