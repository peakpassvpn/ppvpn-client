using System.Runtime.InteropServices;

namespace PPVPN.Windows.Platform;

/// <summary>
/// The system's suspend and resume (PowerRegisterSuspendResumeNotification with a callback, so
/// no window is needed): <c>true</c> at PBT_APMSUSPEND, <c>false</c> at PBT_APMRESUMEAUTOMATIC,
/// which comes on every resume. The callback runs on a system thread; Windows waits about two
/// seconds for it at suspend (host-integration 4.7).
/// </summary>
sealed class SystemSleep : IDisposable
{
    const uint DEVICE_NOTIFY_CALLBACK = 2;
    const uint PBT_APMSUSPEND = 0x4;
    const uint PBT_APMRESUMEAUTOMATIC = 0x12;

    [UnmanagedFunctionPointer(CallingConvention.Winapi)]
    delegate uint DeviceNotifyCallback(IntPtr context, uint type, IntPtr setting);

    [StructLayout(LayoutKind.Sequential)]
    struct DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS
    {
        public DeviceNotifyCallback Callback;
        public IntPtr Context;
    }

    [DllImport("powrprof.dll")]
    static extern uint PowerRegisterSuspendResumeNotification(
        uint flags, ref DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS recipient, out IntPtr handle);

    [DllImport("powrprof.dll")]
    static extern uint PowerUnregisterSuspendResumeNotification(IntPtr handle);

    // Kept alive for as long as Windows may call it.
    readonly DeviceNotifyCallback _callback;
    readonly Func<bool, Task> _changed;
    IntPtr _handle;

    SystemSleep(Func<bool, Task> changed)
    {
        _changed = changed;
        _callback = OnPowerEvent;
    }

    /// <summary>Calls <paramref name="changed"/> at each suspend and resume; null when Windows refuses the registration.</summary>
    public static SystemSleep? Start(Func<bool, Task> changed, Action<string> warn)
    {
        var sleep = new SystemSleep(changed);
        var parameters = new DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback = sleep._callback };
        var error = PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, ref parameters, out sleep._handle);
        if (error == 0) return sleep;
        warn($"system sleep not followed: error {error}");
        return null;
    }

    uint OnPowerEvent(IntPtr context, uint type, IntPtr setting)
    {
        var asleep = type switch
        {
            PBT_APMSUSPEND => true,
            PBT_APMRESUMEAUTOMATIC => false,
            _ => (bool?)null,
        };
        // At suspend, give the cores the time Windows gives this callback.
        if (asleep is { } value) _changed(value).Wait(TimeSpan.FromSeconds(value ? 2 : 0));
        return 0;
    }

    public void Dispose()
    {
        if (_handle == IntPtr.Zero) return;
        PowerUnregisterSuspendResumeNotification(_handle);
        _handle = IntPtr.Zero;
    }
}
