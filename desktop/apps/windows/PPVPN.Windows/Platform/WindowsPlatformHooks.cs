using System.ComponentModel;
using PPVPN.Ffi;

namespace PPVPN.Windows.Platform;

/// <summary>
/// Real Windows implementation of the crate's <see cref="PlatformHooks"/>.
/// Every method may be called on a background thread.
/// </summary>
/// <param name="credentialTarget">
/// Credential Manager entry for the session blob. The fake backend uses its own entry so a
/// fake sign-in never overwrites a real session.
/// </param>
/// <param name="service">
/// <c>--fake-service</c> (fake backend only): pretend the privileged service is installed, or
/// simulate the UAC prompt being allowed or denied, instead of touching the real SCM.
/// </param>
public sealed class WindowsPlatformHooks(string credentialTarget = CredentialStore.Target, FakeService service = FakeService.Real) : PlatformHooks
{
    bool _fakeInstalled = service == FakeService.Installed;

    public byte[]? CredentialLoad()
    {
        try { return CredentialStore.Load(credentialTarget); }
        catch (Win32Exception error) { throw new PlatformException.Failed($"CredRead: {error.Message} ({error.NativeErrorCode})"); }
    }

    public void CredentialSave(byte[] blob)
    {
        try { CredentialStore.Save(blob, credentialTarget); }
        catch (Win32Exception error) { throw new PlatformException.Failed($"CredWrite: {error.Message} ({error.NativeErrorCode})"); }
        catch (ArgumentException error) { throw new PlatformException.Failed(error.Message); }
    }

    public void CredentialDelete()
    {
        try { CredentialStore.Delete(credentialTarget); }
        catch (Win32Exception error) { throw new PlatformException.Failed($"CredDelete: {error.Message} ({error.NativeErrorCode})"); }
    }

    public bool OpenUrl(string url) => Shell.OpenUrl(url);

    public bool PrivilegedServiceInstalled() => service == FakeService.Real ? PrivilegedService.IsInstalled() : _fakeInstalled;

    public void InstallPrivilegedService()
    {
        switch (service)
        {
            case FakeService.Real:
                Check(PrivilegedService.Install());
                return;
            case FakeService.Deny:
                Thread.Sleep(1500);
                throw new PlatformException.Cancelled();
            default:
                Thread.Sleep(1500);
                _fakeInstalled = true;
                return;
        }
    }

    public void UninstallPrivilegedService()
    {
        if (service == FakeService.Real)
        {
            Check(PrivilegedService.Uninstall());
            return;
        }
        Thread.Sleep(1000);
        _fakeInstalled = false;
    }

    static void Check((ServiceActionResult Result, string Message) outcome)
    {
        switch (outcome.Result)
        {
            case ServiceActionResult.Cancelled: throw new PlatformException.Cancelled();
            case ServiceActionResult.Failed: throw new PlatformException.Failed(outcome.Message);
        }
    }
}

/// <summary>How <see cref="WindowsPlatformHooks"/> treats the privileged service (<c>--fake-service</c>).</summary>
public enum FakeService { Real, Installed, Approve, Deny }
