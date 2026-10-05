using System.Reflection;
using System.Runtime.InteropServices;
using Microsoft.Win32;
using PPVPN.Ffi;

namespace PPVPN.PushAgent;

/// <summary>
/// ppvpn-push-agent.exe: pulls backend pushes through the crate's <see cref="PPVPN.Ffi.PushAgent"/>
/// and shows them as PPVPN toasts while the main app is closed.
/// <list type="bullet">
/// <item>One per session: <see cref="InstanceMutex"/>; a second start exits at once.</item>
/// <item>The main app (on sign-out) and the installer set <see cref="QuitEvent"/>: stop, exit.</item>
/// <item>The crate reports Revoked (the push token was rejected): remove our Run value, exit. The
/// main app writes it again and restarts us after it registered the device again.</item>
/// <item>Idle (no push-agent.json): keep running; the crate re-checks the file.</item>
/// <item>Toasts are not shown while they are turned off for PPVPN (the crate keeps those messages
/// pending); the setting is polled every minute and a change to on retries them at once.</item>
/// </list>
/// The main app owns the Run value <see cref="RunValue"/> (PPVPN.Windows/Platform/PushAgentAutostart.cs).
/// </summary>
static partial class Program
{
    public const string InstanceMutex = @"Local\PPVPN.PushAgent";
    public const string QuitEvent = @"Local\PPVPN.PushAgent.Quit";
    const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    public const string RunValue = "PPVPNPushAgent";
    static readonly TimeSpan SettingPoll = TimeSpan.FromSeconds(60);
    static readonly TimeSpan StopTimeout = TimeSpan.FromSeconds(10);

    static int Main()
    {
        var dataDir = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "PPVPN");
        var logDir = Path.Combine(dataDir, "logs");
        var log = new AgentLog(logDir);

        using var instance = new Mutex(initiallyOwned: true, InstanceMutex, out var createdNew);
        if (!createdNew)
        {
            log.Info($"another push agent runs in this session; pid {Environment.ProcessId} exits");
            return 0;
        }
        using var quit = new EventWaitHandle(false, EventResetMode.AutoReset, QuitEvent);

        var version = AppVersion();
        log.Info($"push agent {version} starting, pid {Environment.ProcessId}, {AppContext.BaseDirectory}");
        Toasts.InitializeMta();
        _ = SetCurrentProcessExplicitAppUserModelID(PushNotifier.Aumid);
        var notifier = new PushNotifier(log);
        notifier.RegisterAumid();
        notifier.PollSetting();

        PPVPN.Ffi.PushAgent agent;
        try
        {
            agent = new PPVPN.Ffi.PushAgent(new PushAgentConfig(dataDir, logDir, "windows", version), notifier);
        }
        catch (Exception error)
        {
            // E.g. ppvpn_client.dll missing or not matching the bindings: a broken installation.
            log.Error($"push agent start failed: {error}");
            return 1;
        }
        log.Info($"ppvpn_client loaded from {NativeLoader.LoadedFrom ?? "(default probing)"}");

        using var ended = new ManualResetEvent(false);
        var runner = new Thread(() =>
        {
            try
            {
                agent.Run();
            }
            catch (Exception error)
            {
                log.Error($"push agent run failed: {error}");
            }
            finally
            {
                ended.Set();
            }
        })
        { IsBackground = true, Name = "push-agent" };
        runner.Start();

        var exitCode = 0;
        WaitHandle[] handles = [quit, notifier.Revoked, ended];
        while (true)
        {
            var signaled = WaitHandle.WaitAny(handles, SettingPoll);
            if (signaled == WaitHandle.WaitTimeout)
            {
                if (notifier.PollSetting())
                {
                    log.Info("notifications turned on: retrying pending pushes");
                    agent.RetryPending();
                }
                continue;
            }
            if (signaled == 0)
            {
                log.Info("quit requested");
            }
            else if (signaled == 1)
            {
                log.Info("push token revoked: removing autostart and exiting");
                RemoveRunValue(log);
            }
            else
            {
                log.Error("push agent stopped unexpectedly");
                exitCode = 1;
            }
            break;
        }

        agent.Stop();
        if (!runner.Join(StopTimeout)) log.Warn($"push agent did not stop within {StopTimeout.TotalSeconds:0} s");
        agent.Dispose();
        log.Info("exit");
        return exitCode;
    }

    static void RemoveRunValue(AgentLog log)
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKey, writable: true);
            key?.DeleteValue(RunValue, throwOnMissingValue: false);
        }
        catch (Exception error)
        {
            log.Warn($"removing the Run value failed: {error.Message}");
        }
    }

    /// <summary>VersionPrefix from Directory.Build.props (the informational version without build metadata).</summary>
    static string AppVersion()
    {
        var informational = typeof(Program).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion ?? "0.0.0";
        var plus = informational.IndexOf('+');
        return plus < 0 ? informational : informational[..plus];
    }

    [LibraryImport("shell32.dll", StringMarshalling = StringMarshalling.Utf16)]
    private static partial int SetCurrentProcessExplicitAppUserModelID(string appId);
}
