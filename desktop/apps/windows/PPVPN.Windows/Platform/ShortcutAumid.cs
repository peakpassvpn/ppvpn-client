using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;

namespace PPVPN.Windows.Platform;

/// <summary>
/// Stamps <see cref="AppNotifications.Aumid"/> on a shortcut as System.AppUserModel.ID, so the
/// Start-menu entry, the taskbar button and the app's notifications share one identity (toasts
/// then show the shortcut's name and icon). The installer runs
/// <c>ppvpn.exe --set-shortcut-aumid "&lt;.lnk&gt;"</c> right after creating the shortcut.
/// </summary>
static class ShortcutAumid
{
    public const string Switch = "--set-shortcut-aumid";

    static readonly Guid ShellLinkClsid = new("00021401-0000-0000-C000-000000000046");
    // PKEY_AppUserModel_ID
    static readonly PropertyKey AppUserModelId = new(new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), 5);
    const ushort VT_LPWSTR = 31;
    const int STGM_READWRITE = 2;

    /// <returns>A process exit code: 0 on success.</returns>
    public static int Run(string[] args)
    {
        var index = Array.IndexOf(args, Switch);
        if (index < 0 || index + 1 >= args.Length) return 2;
        try
        {
            Set(args[index + 1], AppNotifications.Aumid);
            return 0;
        }
        catch (Exception)
        {
            return 1;
        }
    }

    public static void Set(string shortcut, string aumid)
    {
        var link = Activator.CreateInstance(Type.GetTypeFromCLSID(ShellLinkClsid, throwOnError: true)!)!;
        try
        {
            var file = (IPersistFile)link;
            file.Load(shortcut, STGM_READWRITE);
            var store = (IPropertyStore)link;
            var value = new PropVariant { vt = VT_LPWSTR, pointer = Marshal.StringToCoTaskMemUni(aumid) };
            try
            {
                var key = AppUserModelId;
                store.SetValue(ref key, ref value);
                store.Commit();
            }
            finally
            {
                PropVariantClear(ref value);
            }
            file.Save(shortcut, true);
        }
        finally
        {
            Marshal.FinalReleaseComObject(link);
        }
    }

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    readonly struct PropertyKey(Guid formatId, uint propertyId)
    {
        readonly Guid _formatId = formatId;
        readonly uint _propertyId = propertyId;
    }

    [StructLayout(LayoutKind.Explicit, Size = 24)]
    struct PropVariant
    {
        [FieldOffset(0)] public ushort vt;
        [FieldOffset(8)] public IntPtr pointer;
    }

    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore
    {
        void GetCount(out uint count);
        void GetAt(uint index, out PropertyKey key);
        void GetValue(ref PropertyKey key, out PropVariant value);
        void SetValue(ref PropertyKey key, ref PropVariant value);
        void Commit();
    }

    [DllImport("ole32.dll", PreserveSig = false)]
    static extern void PropVariantClear(ref PropVariant value);
}
