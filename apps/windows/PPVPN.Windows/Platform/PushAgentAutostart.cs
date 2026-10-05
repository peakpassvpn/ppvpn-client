using System.ComponentModel;
using System.Diagnostics;
using Microsoft.UI.Dispatching;
using Microsoft.Win32;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Windows.Platform;

/// <summary>
/// Starts and stops <c>ppvpn-push-agent.exe</c> (apps/windows/PPVPN.PushAgent), which shows backend
/// pushes as toasts while this app is closed.
/// <list type="bullet">
/// <item>Signed in and the crate has registered the device (it wrote <c>push-agent.json</c>): the
/// per-user Run value <see cref="RunValue"/> points at the agent, and the agent is started when it
/// is not running.</item>
/// <item>Signed out: the Run value is removed and the agent is asked to quit (<see cref="QuitEvent"/>).</item>
/// </list>
/// Separate from launch at sign-in (<see cref="LaunchAtLogin"/>, value <c>PPVPN</c>): the agent runs
/// whether or not the app starts with Windows. The agent itself removes the value when its push
/// token is revoked; the installer removes it on uninstall.
/// </summary>
public sealed class PushAgentAutostart : IDisposable
{
    public const string ExeName = "ppvpn-push-agent.exe";
    /// <summary>Also in installer/ppvpn.nsi.</summary>
    public const string RunValue = "PPVPNPushAgent";
    /// <summary>PPVPN.PushAgent/Program.cs; also in installer/ppvpn.nsi.</summary>
    public const string InstanceMutex = @"Local\PPVPN.PushAgent";
    public const string QuitEvent = @"Local\PPVPN.PushAgent.Quit";
    /// <summary>Written by the crate after registering the device, deleted on sign-out.</summary>
    const string AgentFile = "push-agent.json";
    const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";

    readonly MainViewModel _vm;
    readonly IAppLog _log;
    readonly DispatcherQueue _dispatcher;
    readonly string _agentFile;
    readonly string _agentExe = Path.Combine(AppContext.BaseDirectory, ExeName);
    FileSystemWatcher? _watcher;
    bool _missingLogged;

    PushAgentAutostart(MainViewModel vm, string dataDir, IAppLog log, DispatcherQueue dispatcher)
    {
        _vm = vm;
        _log = log;
        _dispatcher = dispatcher;
        _agentFile = Path.Combine(dataDir, AgentFile);
    }

    /// <summary>Follows the sign-in state and the agent file from now on (call on the UI thread).</summary>
    public static PushAgentAutostart Attach(MainViewModel vm, string dataDir, IAppLog log)
    {
        var autostart = new PushAgentAutostart(vm, dataDir, log, DispatcherQueue.GetForCurrentThread());
        vm.PropertyChanged += autostart.OnViewModelChanged;
        try
        {
            var watcher = new FileSystemWatcher(dataDir, AgentFile) { NotifyFilter = NotifyFilters.FileName | NotifyFilters.LastWrite };
            watcher.Created += autostart.OnFileChanged;
            watcher.Changed += autostart.OnFileChanged;
            watcher.Renamed += autostart.OnFileChanged;
            watcher.Deleted += autostart.OnFileChanged;
            watcher.EnableRaisingEvents = true;
            autostart._watcher = watcher;
        }
        catch (Exception error) when (error is IOException or ArgumentException or UnauthorizedAccessException)
        {
            log.Warn($"push agent: cannot watch {dataDir}: {error.Message}");
        }
        autostart.Evaluate();
        return autostart;
    }

    public void Dispose()
    {
        _vm.PropertyChanged -= OnViewModelChanged;
        _watcher?.Dispose();
        _watcher = null;
    }

    void OnViewModelChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(MainViewModel.Stage)) Evaluate();
    }

    void OnFileChanged(object sender, FileSystemEventArgs e) => _dispatcher.TryEnqueue(Evaluate);

    void Evaluate()
    {
        switch (_vm.Stage)
        {
            case AuthStage.SignedIn when File.Exists(_agentFile):
                Enable();
                break;
            case AuthStage.SignedOut or AuthStage.Awaiting:
                Disable();
                break;
        }
    }

    void Enable()
    {
        if (!File.Exists(_agentExe))
        {
            // A dev build: the agent is not next to ppvpn.exe.
            if (!_missingLogged) _log.Info($"push agent: {_agentExe} not found; not starting it");
            _missingLogged = true;
            return;
        }
        var command = $"\"{_agentExe}\"";
        try
        {
            using var key = Registry.CurrentUser.CreateSubKey(RunKey, writable: true);
            if (key.GetValue(RunValue) as string != command)
            {
                key.SetValue(RunValue, command, RegistryValueKind.String);
                _log.Info("push agent: autostart on");
            }
        }
        catch (Exception error) when (error is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            _log.Warn($"push agent: writing the Run value failed: {error.Message}");
        }
        if (IsRunning()) return;
        try
        {
            using var process = Process.Start(new ProcessStartInfo(_agentExe) { UseShellExecute = false, WorkingDirectory = AppContext.BaseDirectory });
            _log.Info($"push agent: started (pid {process?.Id})");
        }
        catch (Exception error) when (error is Win32Exception or InvalidOperationException)
        {
            _log.Warn($"push agent: start failed: {error.Message}");
        }
    }

    void Disable()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKey, writable: true);
            if (key?.GetValue(RunValue) is not null)
            {
                key.DeleteValue(RunValue, throwOnMissingValue: false);
                _log.Info("push agent: autostart off");
            }
        }
        catch (Exception error) when (error is UnauthorizedAccessException or System.Security.SecurityException or IOException)
        {
            _log.Warn($"push agent: removing the Run value failed: {error.Message}");
        }
        if (EventWaitHandle.TryOpenExisting(QuitEvent, out var quit))
        {
            using (quit) quit.Set();
            _log.Info("push agent: asked to quit");
        }
    }

    static bool IsRunning()
    {
        if (!Mutex.TryOpenExisting(InstanceMutex, out var mutex)) return false;
        mutex.Dispose();
        return true;
    }
}
