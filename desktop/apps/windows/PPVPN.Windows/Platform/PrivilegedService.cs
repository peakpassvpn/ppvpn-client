using System.ComponentModel;
using System.Diagnostics;
using System.Runtime.InteropServices;
using static PPVPN.Windows.Platform.NativeMethods;

namespace PPVPN.Windows.Platform;

public enum ServiceActionResult { Succeeded, Cancelled, Failed }

/// <summary>
/// The enhanced-mode Windows service. Names and helper binaries match
/// service/src/install.rs and installer/ppvpn.nsi.
/// </summary>
public static class PrivilegedService
{
    /// <summary>SERVICE_NAME in service/src/install.rs.</summary>
    public const string ServiceName = "ppvpn_service";

    const string InstallHelper = "ppvpn-service-install.exe";
    const string UninstallHelper = "ppvpn-service-uninstall.exe";

    public static bool IsInstalled()
    {
        var manager = OpenSCManager(null, null, SC_MANAGER_CONNECT);
        if (manager == IntPtr.Zero) return false;
        try
        {
            var service = OpenService(manager, ServiceName, SERVICE_QUERY_STATUS);
            if (service == IntPtr.Zero) return false;
            CloseServiceHandle(service);
            return true;
        }
        finally
        {
            CloseServiceHandle(manager);
        }
    }

    public static (ServiceActionResult Result, string Message) Install() => RunElevated(InstallHelper);

    public static (ServiceActionResult Result, string Message) Uninstall() => RunElevated(UninstallHelper);

    /// <summary>
    /// Launch the helper next to the app exe through the UAC prompt and wait
    /// for it. Blocks; call from a background thread.
    /// </summary>
    static (ServiceActionResult, string) RunElevated(string name)
    {
        var directory = AppContext.BaseDirectory;
        var helper = Path.Combine(directory, name);
        if (!File.Exists(helper))
            return (ServiceActionResult.Failed, $"{name} not found next to {Environment.ProcessPath}");

        try
        {
            using var process = Process.Start(new ProcessStartInfo(helper)
            {
                UseShellExecute = true,
                Verb = "runas",
                WorkingDirectory = directory,
                WindowStyle = ProcessWindowStyle.Hidden,
            });
            if (process is null) return (ServiceActionResult.Failed, $"{Path.GetFileName(helper)} did not start");
            process.WaitForExit();
            return process.ExitCode == 0
                ? (ServiceActionResult.Succeeded, "")
                : (ServiceActionResult.Failed, $"{Path.GetFileName(helper)} exited with {process.ExitCode}");
        }
        catch (Win32Exception error) when (error.NativeErrorCode == ERROR_CANCELLED)
        {
            return (ServiceActionResult.Cancelled, "UAC prompt dismissed");
        }
        catch (Win32Exception error)
        {
            return (ServiceActionResult.Failed, error.Message);
        }
    }
}
