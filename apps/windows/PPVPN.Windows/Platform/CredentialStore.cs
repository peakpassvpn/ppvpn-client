using System.ComponentModel;
using System.Runtime.InteropServices;
using static PPVPN.Windows.Platform.NativeMethods;

namespace PPVPN.Windows.Platform;

/// <summary>
/// The client's single credential blob, stored as ONE generic credential in
/// the Windows Credential Manager (per user, DPAPI-protected by Windows).
/// </summary>
public static class CredentialStore
{
    public const string Target = "PPVPN/desktop.credentials";
    /// <summary>Used by the fake backend (<c>--fake-profile</c>) so it never touches the real session.</summary>
    public const string FakeTarget = "PPVPN/desktop.credentials.fake";
    const string UserName = "PPVPN";

    /// <returns>The blob, or null when there is no saved credential.</returns>
    /// <exception cref="Win32Exception">The store could not be read.</exception>
    public static byte[]? Load(string target = Target)
    {
        if (!CredRead(target, CRED_TYPE_GENERIC, 0, out var pointer))
        {
            var error = Marshal.GetLastWin32Error();
            if (error == ERROR_NOT_FOUND) return null;
            throw new Win32Exception(error);
        }
        try
        {
            var credential = Marshal.PtrToStructure<CREDENTIAL>(pointer);
            var blob = new byte[credential.CredentialBlobSize];
            if (blob.Length > 0) Marshal.Copy(credential.CredentialBlob, blob, 0, blob.Length);
            return blob;
        }
        finally
        {
            CredFree(pointer);
        }
    }

    /// <exception cref="Win32Exception">The store could not be written.</exception>
    /// <exception cref="ArgumentException">The blob exceeds the 2560-byte limit of a generic credential.</exception>
    public static void Save(byte[] blob, string target = Target)
    {
        if (blob.Length > CRED_MAX_CREDENTIAL_BLOB_SIZE)
            throw new ArgumentException($"credential blob is {blob.Length} bytes; the limit is {CRED_MAX_CREDENTIAL_BLOB_SIZE}", nameof(blob));

        var buffer = Marshal.AllocHGlobal(Math.Max(blob.Length, 1));
        try
        {
            Marshal.Copy(blob, 0, buffer, blob.Length);
            var credential = new CREDENTIAL
            {
                Type = CRED_TYPE_GENERIC,
                TargetName = target,
                CredentialBlobSize = (uint)blob.Length,
                CredentialBlob = buffer,
                Persist = CRED_PERSIST_LOCAL_MACHINE,
                UserName = UserName,
            };
            if (!CredWrite(ref credential, 0)) throw new Win32Exception(Marshal.GetLastWin32Error());
        }
        finally
        {
            // Wipe the plaintext copy before releasing it.
            if (blob.Length > 0) Marshal.Copy(new byte[blob.Length], 0, buffer, blob.Length);
            Marshal.FreeHGlobal(buffer);
        }
    }

    /// <summary>Deleting a missing credential is not an error.</summary>
    /// <exception cref="Win32Exception"></exception>
    public static void Delete(string target = Target)
    {
        if (CredDelete(target, CRED_TYPE_GENERIC, 0)) return;
        var error = Marshal.GetLastWin32Error();
        if (error != ERROR_NOT_FOUND) throw new Win32Exception(error);
    }
}
