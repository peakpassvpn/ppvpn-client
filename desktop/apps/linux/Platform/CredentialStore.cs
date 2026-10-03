using System.Runtime.InteropServices;

namespace PPVPN.Linux.Platform;

/// <summary>
/// Backs ppvpn-client's credential_load/save/delete hooks: one opaque blob,
/// one storage entry. Called from Rust threads.
/// </summary>
public interface ICredentialStore
{
    byte[]? Load();
    void Save(byte[] blob);
    void Delete();
}

public static class CredentialStores
{
    public static ICredentialStore Default() =>
        new FallbackCredentialStore(
            new SecretServiceCredentialStore(),
            new FileCredentialStore(Path.Combine(LinuxPaths.DataDir, "credentials")));
}

/// <summary>Thrown when no Secret Service provider is reachable at all.</summary>
public sealed class SecretServiceUnavailableException(string message) : Exception(message);

/// <summary>The keyring holding the saved login is locked; reported to the client as <c>PlatformException.Locked</c>.</summary>
public sealed class KeyringLockedException(string message) : IOException(message);

/// <summary>
/// Secret Service (GNOME Keyring, KWallet, KeePassXC …) first; a 0600 file
/// only when the session has no Secret Service. Any other keyring failure,
/// such as a dismissed unlock prompt, is reported instead of silently
/// writing the credentials to disk.
/// </summary>
public sealed class FallbackCredentialStore(ICredentialStore keyring, ICredentialStore file) : ICredentialStore
{
    private volatile bool _keyringUnavailable;

    public byte[]? Load()
    {
        if (!_keyringUnavailable)
        {
            try
            {
                if (keyring.Load() is { } blob) return blob;
            }
            catch (SecretServiceUnavailableException)
            {
                _keyringUnavailable = true;
            }
        }
        return file.Load();
    }

    public void Save(byte[] blob)
    {
        if (!_keyringUnavailable)
        {
            try
            {
                keyring.Save(blob);
                file.Delete();
                return;
            }
            catch (SecretServiceUnavailableException)
            {
                _keyringUnavailable = true;
            }
        }
        file.Save(blob);
    }

    public void Delete()
    {
        if (!_keyringUnavailable)
        {
            try
            {
                keyring.Delete();
            }
            catch (SecretServiceUnavailableException)
            {
                _keyringUnavailable = true;
            }
        }
        file.Delete();
    }
}

public sealed class FileCredentialStore(string path) : ICredentialStore
{
    public byte[]? Load() => File.Exists(path) ? File.ReadAllBytes(path) : null;

    public void Save(byte[] blob)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var staging = path + ".new";
        // Create with 0600 so the blob is never readable by others, even briefly.
        using (var stream = new FileStream(staging, new FileStreamOptions
        {
            Mode = FileMode.Create,
            Access = FileAccess.Write,
            UnixCreateMode = UnixFileMode.UserRead | UnixFileMode.UserWrite,
        }))
        {
            stream.Write(blob);
            stream.Flush(flushToDisk: true);
        }
        File.SetUnixFileMode(staging, UnixFileMode.UserRead | UnixFileMode.UserWrite);
        File.Move(staging, path, overwrite: true);
    }

    public void Delete()
    {
        if (File.Exists(path)) File.Delete(path);
    }
}

/// <summary>
/// libsecret's binary password API (0.19+). Schema and attributes match the
/// macOS keychain item: service com.peakpassvpn.ppvpn.desktop, account
/// credentials.
/// </summary>
public sealed class SecretServiceCredentialStore : ICredentialStore
{
    private const string SchemaName = "com.peakpassvpn.ppvpn.desktop";
    private const string Account = "credentials";
    private const string Label = "PPVPN";
    // SecretSearchFlags.SECRET_SEARCH_ALL (return every match, locked ones included; no UNLOCK).
    private const int SecretSearchAll = 1 << 1;

    // Set once the unlock prompt was dismissed: later loads (the client retries in the
    // background) report the lock without prompting again, until the keyring is unlocked.
    private volatile bool _unlockDismissed;

    public byte[]? Load() => Call((schema, attributes) =>
    {
        if (_unlockDismissed && LockedItemExists(schema, attributes)) throw KeyringLocked();
        var value = Native.secret_password_lookupv_binary_sync(schema, attributes, IntPtr.Zero, out var error);
        ThrowIfError(error);
        if (value == IntPtr.Zero)
        {
            // Nothing came back: nothing is saved, or the saved login sits in a locked
            // keyring whose unlock prompt was dismissed. Only the first means signed out;
            // the second is an unavailable store, so the client keeps the login and retries.
            if (!LockedItemExists(schema, attributes)) return null;
            _unlockDismissed = true;
            throw KeyringLocked();
        }
        _unlockDismissed = false;
        try
        {
            var data = Native.secret_value_get(value, out var length);
            var blob = new byte[(int)length];
            Marshal.Copy(data, blob, 0, blob.Length);
            return blob;
        }
        finally
        {
            Native.secret_value_unref(value);
        }
    });

    private static KeyringLockedException KeyringLocked() =>
        new("LINUX_KEYRING_LOCKED: the keyring holding the saved login is locked");

    /// <summary>A matching item exists and is locked (searched without unlocking).</summary>
    private static bool LockedItemExists(IntPtr schema, IntPtr attributes)
    {
        var list = Native.secret_service_search_sync(IntPtr.Zero, schema, attributes, SecretSearchAll, IntPtr.Zero, out var error);
        ThrowIfError(error);
        try
        {
            // GList: { gpointer data; GList *next; GList *prev; }
            for (var node = list; node != IntPtr.Zero; node = Marshal.ReadIntPtr(node, IntPtr.Size))
            {
                if (Native.secret_item_get_locked(Marshal.ReadIntPtr(node)) != 0) return true;
            }
            return false;
        }
        finally
        {
            if (list != IntPtr.Zero) Native.g_list_free_full(list, Native.GObjectUnref);
        }
    }

    public void Save(byte[] blob) => Call<object?>((schema, attributes) =>
    {
        var buffer = Marshal.AllocHGlobal(Math.Max(blob.Length, 1));
        try
        {
            Marshal.Copy(blob, 0, buffer, blob.Length);
            // secret_value_new copies the buffer.
            var value = Native.secret_value_new(buffer, blob.Length, "application/octet-stream");
            try
            {
                Native.secret_password_storev_binary_sync(
                    schema, attributes, null, Label, value, IntPtr.Zero, out var error);
                ThrowIfError(error);
            }
            finally
            {
                Native.secret_value_unref(value);
            }
        }
        finally
        {
            Marshal.FreeHGlobal(buffer);
        }
        return null;
    });

    public void Delete() => Call<object?>((schema, attributes) =>
    {
        Native.secret_password_clearv_sync(schema, attributes, IntPtr.Zero, out var error);
        ThrowIfError(error);
        return null;
    });

    private static T Call<T>(Func<IntPtr, IntPtr, T> body)
    {
        IntPtr schemaTypes = IntPtr.Zero, schema = IntPtr.Zero, attributes = IntPtr.Zero;
        var strings = new List<IntPtr>();
        IntPtr Utf8(string text)
        {
            var pointer = Marshal.StringToCoTaskMemUTF8(text);
            strings.Add(pointer);
            return pointer;
        }

        try
        {
            schemaTypes = Native.NewStringTable();
            // Value 0 is SECRET_SCHEMA_ATTRIBUTE_STRING.
            Native.g_hash_table_insert(schemaTypes, Utf8("service"), IntPtr.Zero);
            Native.g_hash_table_insert(schemaTypes, Utf8("account"), IntPtr.Zero);
            schema = Native.secret_schema_newv(SchemaName, 0, schemaTypes);

            attributes = Native.NewStringTable();
            Native.g_hash_table_insert(attributes, Utf8("service"), Utf8(SchemaName));
            Native.g_hash_table_insert(attributes, Utf8("account"), Utf8(Account));
            return body(schema, attributes);
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException)
        {
            throw new SecretServiceUnavailableException($"LINUX_LIBSECRET_MISSING:{error.Message}");
        }
        finally
        {
            if (attributes != IntPtr.Zero) Native.g_hash_table_unref(attributes);
            if (schema != IntPtr.Zero) Native.secret_schema_unref(schema);
            if (schemaTypes != IntPtr.Zero) Native.g_hash_table_unref(schemaTypes);
            strings.ForEach(Marshal.FreeCoTaskMem);
        }
    }

    // GDBusError values that mean nobody provides org.freedesktop.secrets.
    private const int DBusErrorServiceUnknown = 2;
    private const int DBusErrorNameHasNoOwner = 3;

    private static void ThrowIfError(IntPtr error)
    {
        if (error == IntPtr.Zero) return;
        var info = Marshal.PtrToStructure<Native.GError>(error);
        var message = Marshal.PtrToStringUTF8(info.message) ?? "unknown error";
        Native.g_error_free(error);

        var noProvider = info.domain == Native.g_dbus_error_quark()
            && info.code is DBusErrorServiceUnknown or DBusErrorNameHasNoOwner;
        // A GIOError here means there is no session bus to talk to.
        if (noProvider || info.domain == Native.g_io_error_quark())
            throw new SecretServiceUnavailableException($"LINUX_SECRET_SERVICE_UNAVAILABLE:{message}");
        throw new IOException($"LINUX_SECRET_SERVICE_FAILED:{message}");
    }

    private static class Native
    {
        private const string Secret = "libsecret-1.so.0";
        private const string GLib = "libglib-2.0.so.0";
        private const string Gio = "libgio-2.0.so.0";

        [StructLayout(LayoutKind.Sequential)]
        public struct GError
        {
            public uint domain;
            public int code;
            public IntPtr message;
        }

        private static readonly Lazy<(IntPtr Hash, IntPtr Equal)> StringFunctions = new(() =>
        {
            var glib = NativeLibrary.Load(GLib);
            return (NativeLibrary.GetExport(glib, "g_str_hash"), NativeLibrary.GetExport(glib, "g_str_equal"));
        });

        /// <summary>A GHashTable keyed by C strings; the caller owns keys and values.</summary>
        public static IntPtr NewStringTable() =>
            g_hash_table_new_full(StringFunctions.Value.Hash, StringFunctions.Value.Equal, IntPtr.Zero, IntPtr.Zero);

        [DllImport(GLib)] public static extern IntPtr g_hash_table_new_full(IntPtr hash, IntPtr equal, IntPtr keyDestroy, IntPtr valueDestroy);
        [DllImport(GLib)] public static extern int g_hash_table_insert(IntPtr table, IntPtr key, IntPtr value);
        [DllImport(GLib)] public static extern void g_hash_table_unref(IntPtr table);
        [DllImport(GLib)] public static extern void g_error_free(IntPtr error);
        [DllImport(Gio)] public static extern uint g_dbus_error_quark();
        [DllImport(Gio)] public static extern uint g_io_error_quark();

        [DllImport(Secret)] public static extern IntPtr secret_schema_newv(
            [MarshalAs(UnmanagedType.LPUTF8Str)] string name, int flags, IntPtr attributeTypes);
        [DllImport(Secret)] public static extern void secret_schema_unref(IntPtr schema);
        [DllImport(Secret)] public static extern IntPtr secret_value_new(
            IntPtr secret, nint length, [MarshalAs(UnmanagedType.LPUTF8Str)] string contentType);
        [DllImport(Secret)] public static extern IntPtr secret_value_get(IntPtr value, out nuint length);
        [DllImport(Secret)] public static extern void secret_value_unref(IntPtr value);
        [DllImport(Secret)] public static extern int secret_password_storev_binary_sync(
            IntPtr schema, IntPtr attributes, [MarshalAs(UnmanagedType.LPUTF8Str)] string? collection,
            [MarshalAs(UnmanagedType.LPUTF8Str)] string label, IntPtr value, IntPtr cancellable, out IntPtr error);
        [DllImport(Secret)] public static extern IntPtr secret_password_lookupv_binary_sync(
            IntPtr schema, IntPtr attributes, IntPtr cancellable, out IntPtr error);
        [DllImport(Secret)] public static extern int secret_password_clearv_sync(
            IntPtr schema, IntPtr attributes, IntPtr cancellable, out IntPtr error);
        [DllImport(Secret)] public static extern IntPtr secret_service_search_sync(
            IntPtr service, IntPtr schema, IntPtr attributes, int flags, IntPtr cancellable, out IntPtr error);
        [DllImport(Secret)] public static extern int secret_item_get_locked(IntPtr item);
        [DllImport(GLib)] public static extern void g_list_free_full(IntPtr list, IntPtr freeFunc);

        /// <summary>g_object_unref, to free the GList of SecretItems.</summary>
        public static IntPtr GObjectUnref => ObjectUnref.Value;
        private static readonly Lazy<IntPtr> ObjectUnref = new(() =>
            NativeLibrary.GetExport(NativeLibrary.Load("libgobject-2.0.so.0"), "g_object_unref"));
    }
}
