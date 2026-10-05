using System.Diagnostics;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.Windows.AppLifecycle;
using PPVPN.Windows.Platform;

namespace PPVPN.Windows;

/// <summary>
/// Custom entry point (DISABLE_XAML_GENERATED_MAIN) so a second launch can
/// hand its activation to the running instance before any UI starts, and so
/// notification registration happens before the activation is read.
/// </summary>
public static class Program
{
    const string InstanceKey = "PPVPN.Desktop.Main";

    /// <summary>
    /// Raised on a background thread when another launch was redirected here (a notification
    /// click forwarded that way goes to <see cref="AppNotifications"/> instead).
    /// </summary>
    public static event Action? Reactivated;

    [STAThread]
    static int Main(string[] args)
    {
        // Installer helper: stamp our AppUserModelID on its shortcut, then exit.
        if (args.Contains(ShortcutAumid.Switch)) return ShortcutAumid.Run(args);

        AppNotifications.SetProcessAumid();
        WinRT.ComWrappersSupport.InitializeComWrappers();
        AppNotifications.RegisterAumid();
        if (RedirectToRunningInstance(args)) return 0;

        // This is the running instance: notification clicks come here from now on (queued
        // until App attaches), including the one COM started us for, or one forwarded to us.
        AppNotifications.RegisterActivator();
        if (AppNotifications.TryParseForward(Environment.CommandLine, out var forwarded)) AppNotifications.Deliver(forwarded);

        Application.Start(p =>
        {
            var context = new DispatcherQueueSynchronizationContext(DispatcherQueue.GetForCurrentThread());
            SynchronizationContext.SetSynchronizationContext(context);
            _ = new App();
        });
        return 0;
    }

    /// <summary>
    /// <c>--fake-profile</c> runs are their own instance (and keep their own session, see
    /// CredentialStore.FakeTarget), so a fake run for screenshots does not hand its activation to
    /// an installed app that is running in the same session.
    /// </summary>
    static string KeyFor(string[] args) =>
        args.Any(a => a.StartsWith("--fake-profile", StringComparison.Ordinal)) ? InstanceKey + ".Fake" : InstanceKey;

    /// <returns>True when another instance owns the key and got our activation.</returns>
    static bool RedirectToRunningInstance(string[] args)
    {
        var key = KeyFor(args);
        for (var attempt = 1; ; attempt++)
        {
            var main = AppInstance.FindOrRegisterForKey(key);
            if (main.IsCurrent)
            {
                if (attempt > 1) StartupLog($"single instance: registered as {key} on attempt {attempt}");
                main.Activated += (_, activation) =>
                {
                    // A second launch; it carries a notification click when it forwarded one.
                    var commandLine = activation.Data is global::Windows.ApplicationModel.Activation.ILaunchActivatedEventArgs launch ? launch.Arguments : null;
                    if (AppNotifications.TryParseForward(commandLine, out var click)) AppNotifications.Deliver(click);
                    else Reactivated?.Invoke();
                };
                return false;
            }

            if (args.Contains(AppNotifications.ToastActivatedSwitch))
            {
                // COM started us for a notification click although an instance is running: take the
                // click and hand it over on the command line of a launch that redirects like any other.
                if (AppNotifications.WaitForActivation(TimeSpan.FromSeconds(10)) is { } click && Environment.ProcessPath is { } exe)
                    Process.Start(new ProcessStartInfo(exe, AppNotifications.ForwardArgument(click)) { UseShellExecute = false });
                return true;
            }

            if (TryRedirect(main)) return true;

            // The owner did not take the activation within the timeout: it is exiting (e.g. killed
            // a moment ago, its key not yet released) or hung. Wait for it to go away, then try to
            // become the main instance ourselves instead of hanging with no window.
            var exited = WaitForExit(main.ProcessId, TimeSpan.FromSeconds(3));
            StartupLog($"single instance: pid {main.ProcessId} did not accept the activation (attempt {attempt}, {(exited ? "it exited" : "still running")})");
            if (attempt >= 3)
            {
                StartupLog("single instance: giving up; exiting");
                return true;
            }
        }
    }

    /// <summary>Redirects this launch to <paramref name="main"/>; false when it did not complete within 5 s.</summary>
    static bool TryRedirect(AppInstance main)
    {
        // Let the running instance bring its window to the foreground.
        NativeMethods.AllowSetForegroundWindow(main.ProcessId);
        var activated = AppInstance.GetCurrent().GetActivatedEventArgs();

        // RedirectActivationToAsync must not block the STA without pumping COM; wait on an event
        // with CoWaitForMultipleObjects (the Windows App SDK single-instancing sample), bounded:
        // an owner that is exiting never answers.
        var done = NativeMethods.CreateEvent(IntPtr.Zero, true, false, null);
        var succeeded = false;
        _ = Task.Run(() =>
        {
            try
            {
                main.RedirectActivationToAsync(activated).AsTask().Wait();
                succeeded = true;
            }
            catch (Exception) { }
            finally { NativeMethods.SetEvent(done); }
        });
        var result = NativeMethods.CoWaitForMultipleObjects(NativeMethods.CWMO_DEFAULT, 5000, 1, [done], out _);
        const uint RPC_S_CALLPENDING = 0x80010115;
        if (result == RPC_S_CALLPENDING) return false; // timed out; the event stays open for the late SetEvent
        NativeMethods.CloseHandle(done);
        return succeeded;
    }

    static bool WaitForExit(uint processId, TimeSpan timeout)
    {
        try
        {
            using var process = Process.GetProcessById((int)processId);
            return process.WaitForExit(timeout);
        }
        catch (ArgumentException)
        {
            return true; // already gone
        }
        catch (InvalidOperationException)
        {
            return true;
        }
    }

    /// <summary>Before the app log exists: append to the same file (UTC date).</summary>
    static void StartupLog(string message)
    {
        try
        {
            var directory = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "PPVPN", "logs");
            Directory.CreateDirectory(directory);
            var now = DateTimeOffset.Now;
            File.AppendAllText(Path.Combine(directory, $"ppvpn-windows.{now.UtcDateTime:yyyy-MM-dd}.log"),
                $"{now.ToString("yyyy-MM-ddTHH:mm:ss.fffzzz", System.Globalization.CultureInfo.InvariantCulture)} WARN ppvpn_windows: {message}{Environment.NewLine}");
        }
        catch (Exception) { }
    }
}
