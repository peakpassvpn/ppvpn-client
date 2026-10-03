using System.Diagnostics;
using PPVPN.Ffi;

namespace PPVPN.Linux.Platform;

/// <summary>Linux side of ppvpn-client's <c>PlatformHooks</c>. Called from Rust threads.</summary>
public sealed class LinuxPlatformHooks(ICredentialStore credentials) : PlatformHooks
{
    public LinuxPlatformHooks() : this(CredentialStores.Default()) { }

    // Credentials

    public byte[]? CredentialLoad() => Guard(credentials.Load);

    public void CredentialSave(byte[] blob) => Guard(() => credentials.Save(blob));

    public void CredentialDelete() => Guard(credentials.Delete);

    private static T Guard<T>(Func<T> body)
    {
        try { return body(); }
        catch (KeyringLockedException error) { throw new PlatformException.Locked(error.Message); }
        catch (Exception error) { throw new PlatformException.Failed(error.Message); }
    }

    private static void Guard(Action body) => Guard<object?>(() => { body(); return null; });

    // Browser

    public bool OpenUrl(string url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.Scheme is "https" or "http"
            && LinuxAppServices.XdgOpen(uri.AbsoluteUri);

    // Privileged service. Paths are owned by service/src/install.rs.

    private const string UnitPath = "/etc/systemd/system/ppvpn-service.service";
    private const string ServiceBinary = "/usr/lib/ppvpn-service/ppvpn-service";

    public bool PrivilegedServiceInstalled() => File.Exists(UnitPath) && File.Exists(ServiceBinary);

    public void InstallPrivilegedService() => RunHelperAsAdmin("ppvpn-service-install");

    public void UninstallPrivilegedService() => RunHelperAsAdmin("ppvpn-service-uninstall");

    // pkexec exit codes: 126 = the authentication dialog was dismissed,
    // 127 = not authorized or no polkit agent is running.
    private const int PkexecDismissed = 126;
    private const int PkexecNotAuthorized = 127;

    /// <summary>
    /// Runs a helper installed next to the app as root behind the polkit
    /// password prompt.
    /// </summary>
    private static void RunHelperAsAdmin(string name)
    {
        var helper = Path.Combine(LinuxPaths.AppDir, name);
        if (!File.Exists(helper)) throw new PlatformException.Failed($"LINUX_PRIVILEGED_HELPER_MISSING:{name}");

        var start = new ProcessStartInfo("pkexec")
        {
            ArgumentList = { helper },
            RedirectStandardError = true,
            RedirectStandardOutput = true,
        };
        Process? process;
        try
        {
            process = Process.Start(start);
        }
        catch (System.ComponentModel.Win32Exception)
        {
            throw new PlatformException.Failed("LINUX_PKEXEC_MISSING");
        }
        if (process is null) throw new PlatformException.Failed("LINUX_PKEXEC_MISSING");

        using (process)
        {
            // Drain stdout so a chatty helper cannot block on a full pipe.
            _ = process.StandardOutput.ReadToEndAsync();
            var stderr = process.StandardError.ReadToEnd().Trim();
            process.WaitForExit();
            switch (process.ExitCode)
            {
                case 0:
                    return;
                case PkexecDismissed:
                    throw new PlatformException.Cancelled();
                case PkexecNotAuthorized when stderr.Contains("authentication agent", StringComparison.OrdinalIgnoreCase):
                    throw new PlatformException.Failed($"LINUX_POLKIT_AGENT_MISSING:{stderr}");
                case PkexecNotAuthorized:
                    throw new PlatformException.Failed($"LINUX_ADMIN_AUTHORIZATION_FAILED:{stderr}");
                default:
                    // The helper reports LINUX_SYSTEMD_UNAVAILABLE itself.
                    throw new PlatformException.Failed($"LINUX_SERVICE_HELPER_FAILED:{name}:{stderr}");
            }
        }
    }
}
