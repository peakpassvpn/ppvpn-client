using System.Runtime.InteropServices;

namespace PPVPN.PushAgent;

/// <summary>
/// Classic Windows toasts (<c>Windows.UI.Notifications</c>) through raw WinRT COM calls: activation
/// factories from combase and a handful of vtable slots. No CsWinRT projection, so nothing here
/// needs reflection or runtime code generation and it is NativeAOT-safe by construction.
/// <para>
/// Equivalent of <c>ToastNotificationManager.CreateToastNotifier(aumid).Show(new ToastNotification(xml)
/// { Tag, Group })</c> and <c>.Setting</c> in the main app (PPVPN.Windows/Platform/AppNotifications.cs).
/// Slots count from 6: IUnknown (3) and IInspectable (3) come first.
/// </para>
/// Call from MTA threads (the agent initializes the MTA on its main thread, so the crate's
/// callback threads use the implicit MTA).
/// </summary>
static unsafe partial class Toasts
{
    /// <summary>Windows.UI.Notifications.NotificationSetting</summary>
    public enum NotificationSetting
    {
        Enabled = 0,
        DisabledForApplication = 1,
        DisabledForUser = 2,
        DisabledByGroupPolicy = 3,
        DisabledByManifest = 4,
    }

    static readonly Guid IidToastNotificationManagerStatics = new("50AC103F-D235-4598-BBEF-98FE4D1A3AD4");
    static readonly Guid IidToastNotificationFactory = new("04124B20-82C6-4229-B109-FD9ED4662B53");
    static readonly Guid IidToastNotification2 = new("9DFB9FD1-143A-490E-90BF-B9FBA7132DE7");
    static readonly Guid IidXmlDocument = new("F7F3A506-1E87-42D6-BCFB-B8C809FA5494");
    static readonly Guid IidXmlDocumentIO = new("6CD0E74E-EE65-4489-9EBF-CA43E87BA637");

    /// <summary>Joins the process to the MTA (kept for the process lifetime).</summary>
    public static void InitializeMta()
    {
        const int RO_INIT_MULTITHREADED = 1;
        _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    /// <summary>The per-app setting in Settings › System › Notifications (or policy).</summary>
    public static NotificationSetting Setting(string aumid)
    {
        var notifier = CreateNotifier(aumid);
        try
        {
            int value;
            Check(((delegate* unmanaged<IntPtr, int*, int>)Slot(notifier, 8))(notifier, &value), "IToastNotifier.get_Setting");
            return (NotificationSetting)value;
        }
        finally
        {
            Release(notifier);
        }
    }

    /// <summary>Shows a toast built from <paramref name="xml"/>; throws <see cref="COMException"/> on failure.</summary>
    public static void Show(string aumid, string xml, string tag, string group)
    {
        IntPtr inspectable = 0, io = 0, document = 0, factory = 0, toast = 0, toast2 = 0, notifier = 0;
        try
        {
            using (var name = new HString("Windows.Data.Xml.Dom.XmlDocument"))
                Check(RoActivateInstance(name.Handle, out inspectable), "RoActivateInstance(XmlDocument)");
            io = QueryInterface(inspectable, IidXmlDocumentIO);
            using (var content = new HString(xml))
                Check(((delegate* unmanaged<IntPtr, IntPtr, int>)Slot(io, 6))(io, content.Handle), "IXmlDocumentIO.LoadXml");
            document = QueryInterface(inspectable, IidXmlDocument);

            factory = Factory("Windows.UI.Notifications.ToastNotification", IidToastNotificationFactory);
            IntPtr created;
            Check(((delegate* unmanaged<IntPtr, IntPtr, IntPtr*, int>)Slot(factory, 6))(factory, document, &created), "IToastNotificationFactory.CreateToastNotification");
            toast = created;

            toast2 = QueryInterface(toast, IidToastNotification2);
            using (var value = new HString(tag))
                Check(((delegate* unmanaged<IntPtr, IntPtr, int>)Slot(toast2, 6))(toast2, value.Handle), "IToastNotification2.put_Tag");
            using (var value = new HString(group))
                Check(((delegate* unmanaged<IntPtr, IntPtr, int>)Slot(toast2, 8))(toast2, value.Handle), "IToastNotification2.put_Group");

            notifier = CreateNotifier(aumid);
            Check(((delegate* unmanaged<IntPtr, IntPtr, int>)Slot(notifier, 6))(notifier, toast), "IToastNotifier.Show");
        }
        finally
        {
            Release(notifier);
            Release(toast2);
            Release(toast);
            Release(factory);
            Release(document);
            Release(io);
            Release(inspectable);
        }
    }

    static IntPtr CreateNotifier(string aumid)
    {
        var statics = Factory("Windows.UI.Notifications.ToastNotificationManager", IidToastNotificationManagerStatics);
        try
        {
            using var id = new HString(aumid);
            IntPtr notifier;
            Check(((delegate* unmanaged<IntPtr, IntPtr, IntPtr*, int>)Slot(statics, 7))(statics, id.Handle, &notifier), "CreateToastNotifierWithId");
            return notifier;
        }
        finally
        {
            Release(statics);
        }
    }

    static IntPtr Factory(string runtimeClass, Guid iid)
    {
        using var name = new HString(runtimeClass);
        Check(RoGetActivationFactory(name.Handle, &iid, out var factory), $"RoGetActivationFactory({runtimeClass})");
        return factory;
    }

    static IntPtr QueryInterface(IntPtr unknown, Guid iid)
    {
        IntPtr result;
        Check(((delegate* unmanaged<IntPtr, Guid*, IntPtr*, int>)Slot(unknown, 0))(unknown, &iid, &result), "QueryInterface");
        return result;
    }

    static void Release(IntPtr unknown)
    {
        if (unknown != 0) ((delegate* unmanaged<IntPtr, uint>)Slot(unknown, 2))(unknown);
    }

    static void* Slot(IntPtr instance, int slot) => (*(void***)instance)[slot];

    static void Check(int hr, string what)
    {
        if (hr < 0) throw new COMException($"{what} failed: 0x{hr:X8}", hr);
    }

    sealed class HString : IDisposable
    {
        public HString(string value)
        {
            IntPtr handle;
            fixed (char* chars = value)
                Check(WindowsCreateString(chars, (uint)value.Length, out handle), "WindowsCreateString");
            Handle = handle;
        }

        public IntPtr Handle { get; private set; }

        public void Dispose()
        {
            if (Handle != 0) _ = WindowsDeleteString(Handle);
            Handle = 0;
        }
    }

    [LibraryImport("combase.dll")]
    private static partial int RoInitialize(int initType);

    [LibraryImport("combase.dll")]
    private static partial int WindowsCreateString(char* sourceString, uint length, out IntPtr hstring);

    [LibraryImport("combase.dll")]
    private static partial int WindowsDeleteString(IntPtr hstring);

    [LibraryImport("combase.dll")]
    private static partial int RoGetActivationFactory(IntPtr activatableClassId, Guid* iid, out IntPtr factory);

    [LibraryImport("combase.dll")]
    private static partial int RoActivateInstance(IntPtr activatableClassId, out IntPtr instance);
}
