using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Security;
using Microsoft.Win32;
using WinToast = global::Windows.UI.Notifications;
using WinXml = global::Windows.Data.Xml.Dom;

namespace PPVPN.Windows.Platform;

/// <summary>A click on one of our notifications: the push id (<c>PushMessage.Id</c>), nothing else.</summary>
public sealed record NotificationActivation(ulong Id);

/// <summary>
/// Toast notifications for this unpackaged, self-contained app, with click activation.
/// <para>
/// Windows App SDK's <c>AppNotificationManager.Register</c> fails here with 0x8007007E: for
/// unpackaged apps it needs the Windows App Runtime "Singleton" package (its notification
/// long-running process), which a self-contained app does not install. So this is the classic
/// desktop route that <c>AppNotificationManager</c> itself uses underneath:
/// </para>
/// <list type="bullet">
/// <item><see cref="Aumid"/> is the process's explicit AppUserModelID (set first thing in
/// <c>Main</c>); the installer stamps it on the Start-menu shortcut (<see cref="ShortcutAumid"/>).</item>
/// <item><see cref="RegisterAumid"/> writes <c>HKCU\Software\Classes\AppUserModelId\PeakPass.PPVPN</c>
/// (display name, icon, <c>CustomActivator</c>) and the activator's <c>LocalServer32</c> (this exe
/// with <see cref="ToastActivatedSwitch"/>), so toasts show "PPVPN" with our icon and a click
/// reaches us even when the app is not running (cold start).</item>
/// <item><see cref="RegisterActivator"/> registers the <c>INotificationActivationCallback</c>
/// class object in the running (single) instance: clicks arrive there, on the UI thread.</item>
/// <item>A launch that loses the single-instance race after COM started it for a click receives
/// the click itself (<see cref="WaitForActivation"/>) and forwards it on the command line
/// (<see cref="ForwardSwitch"/>) through the normal single-instance redirection.</item>
/// </list>
/// Everything ends in the handler installed with <see cref="Attach"/>; clicks before that are queued.
/// The registration stays after quitting (a toast in the notification center still starts the
/// app); the uninstaller removes it.
/// </summary>
public static class AppNotifications
{
    /// <summary>Also in installer/ppvpn.nsi (shortcut stamp, uninstall cleanup).</summary>
    public const string Aumid = "PeakPass.PPVPN";
    public const string DisplayName = "PPVPN";
    /// <summary>CLSID of our <c>INotificationActivationCallback</c> (the AUMID's CustomActivator; also in installer/ppvpn.nsi).</summary>
    public const string ActivatorClsid = "FCD3C3FA-FCA6-4F2D-BC9E-BEA74D70EBBE";
    /// <summary>COM starts the app with this (and <c>-Embedding</c>) to deliver a click.</summary>
    public const string ToastActivatedSwitch = "--toast-activated";
    /// <summary>A click forwarded to the running instance on the command line.</summary>
    public const string ForwardSwitch = "--notification-activated=";

    /// <summary>The toast's launch arguments are <c>id=&lt;pushId&gt;</c> (the push agent writes them).</summary>
    const string IdArgument = "id";

    static readonly object Gate = new();
    static readonly List<NotificationActivation> Pending = [];
    static Action<NotificationActivation>? _handler;
    static ActivatorFactory? _factory;

    public static bool IsRegistered { get; private set; }

    /// <summary>Why registration failed, for the log (the app log does not exist yet in <c>Main</c>).</summary>
    public static string? RegistrationError { get; private set; }

    /// <summary>Taskbar grouping and notifications use this id.</summary>
    public static void SetProcessAumid() => SetCurrentProcessExplicitAppUserModelID(Aumid);

    /// <summary>Per-user registration of the AUMID (name, icon) and of this exe as the click activator.</summary>
    public static void RegisterAumid()
    {
        try
        {
            var exe = Environment.ProcessPath ?? Path.Combine(AppContext.BaseDirectory, "ppvpn.exe");
            using (var key = Registry.CurrentUser.CreateSubKey($@"Software\Classes\AppUserModelId\{Aumid}"))
            {
                key.SetValue("DisplayName", DisplayName);
                // Only an existing file: the notification platform caches the icon per AUMID, and
                // a missing one stays blank until WpnUserService restarts.
                var icon = Path.Combine(AppContext.BaseDirectory, "Assets", "AppLogo.png");
                if (File.Exists(icon)) key.SetValue("IconUri", icon);
                key.SetValue("CustomActivator", $"{{{ActivatorClsid}}}");
            }
            using (var server = Registry.CurrentUser.CreateSubKey($@"Software\Classes\CLSID\{{{ActivatorClsid}}}\LocalServer32"))
                server.SetValue("", $"\"{exe}\" {ToastActivatedSwitch}");
        }
        catch (Exception error)
        {
            RegistrationError = $"registry: {error.Message}";
        }
    }

    /// <summary>Receive clicks in this process from now on (call on the STA UI thread).</summary>
    public static void RegisterActivator()
    {
        if (_factory is not null) return;
        _factory = new ActivatorFactory();
        var clsid = new Guid(ActivatorClsid);
        var hr = CoRegisterClassObject(ref clsid, _factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE, out _);
        if (hr < 0) RegistrationError ??= $"CoRegisterClassObject: 0x{hr:X8}";
        else IsRegistered = RegistrationError is null;
    }

    /// <summary>
    /// For a launch COM started for a click while another instance runs: take the click here
    /// (pumping COM on this STA for up to <paramref name="timeout"/>).
    /// </summary>
    public static NotificationActivation? WaitForActivation(TimeSpan timeout)
    {
        NotificationActivation? received = null;
        var done = NativeMethods.CreateEvent(IntPtr.Zero, true, false, null);
        lock (Gate)
        {
            _handler = activation =>
            {
                received = activation;
                NativeMethods.SetEvent(done);
            };
        }
        RegisterActivator();
        NativeMethods.CoWaitForMultipleObjects(CWMO_DISPATCH_CALLS | CWMO_DISPATCH_WINDOW_MESSAGES, (uint)timeout.TotalMilliseconds, 1, [done], out _);
        lock (Gate) _handler = null;
        NativeMethods.CloseHandle(done);
        return received;
    }

    public static string ForwardArgument(NotificationActivation activation) =>
        ForwardSwitch + Uri.EscapeDataString(Encode(activation));

    /// <summary>Finds a forwarded click in a command line (ours, or one redirected to us).</summary>
    public static bool TryParseForward(string? commandLine, out NotificationActivation activation)
    {
        activation = null!;
        var start = commandLine?.IndexOf(ForwardSwitch, StringComparison.Ordinal) ?? -1;
        if (start < 0) return false;
        var value = commandLine![(start + ForwardSwitch.Length)..];
        var end = value.IndexOfAny([' ', '"']);
        if (end >= 0) value = value[..end];
        return TryDecode(Uri.UnescapeDataString(value), out activation);
    }

    /// <summary>A click from the activator, a forwarded command line or a redirected launch.</summary>
    public static void Deliver(NotificationActivation activation)
    {
        Action<NotificationActivation>? handler;
        lock (Gate)
        {
            handler = _handler;
            if (handler is null) Pending.Add(activation);
        }
        handler?.Invoke(activation);
    }

    /// <summary>Receives every click from now on (on any thread), after the ones queued so far.</summary>
    public static void Attach(Action<NotificationActivation> handler)
    {
        NotificationActivation[] queued;
        lock (Gate)
        {
            _handler = handler;
            queued = [.. Pending];
            Pending.Clear();
        }
        foreach (var activation in queued) handler(activation);
    }

    /// <summary>A plain notice with no message behind it (first close: "still running").</summary>
    internal static void ShowNotice(string title, string body)
    {
        var xml = new WinXml.XmlDocument();
        xml.LoadXml(
            "<toast><visual><binding template=\"ToastGeneric\">" +
            $"<text>{SecurityElement.Escape(title)}</text><text>{SecurityElement.Escape(body)}</text>" +
            "</binding></visual></toast>");
        WinToast.ToastNotificationManager.CreateToastNotifier(Aumid).Show(new WinToast.ToastNotification(xml) { Tag = "notice", Group = "app" });
    }

    static string Encode(NotificationActivation activation) =>
        $"{IdArgument}={activation.Id.ToString(CultureInfo.InvariantCulture)}";

    static bool TryDecode(string arguments, out NotificationActivation activation)
    {
        activation = null!;
        ulong? id = null;
        foreach (var pair in arguments.Split('&', StringSplitOptions.RemoveEmptyEntries))
        {
            var (name, value) = pair.Split('=', 2) is [var n, var v] ? (n, Uri.UnescapeDataString(v)) : (pair, "");
            if (name == IdArgument && ulong.TryParse(value, NumberStyles.None, CultureInfo.InvariantCulture, out var parsed)) id = parsed;
        }
        if (id is null) return false;
        activation = new NotificationActivation(id.Value);
        return true;
    }

    // --- COM activator ---------------------------------------------------

    const uint CLSCTX_LOCAL_SERVER = 4;
    const uint REGCLS_MULTIPLEUSE = 1;
    const uint CWMO_DISPATCH_CALLS = 1;
    const uint CWMO_DISPATCH_WINDOW_MESSAGES = 2;
    const int E_NOINTERFACE = unchecked((int)0x80004002);
    const int CLASS_E_NOAGGREGATION = unchecked((int)0x80040110);
    static readonly Guid IUnknownIid = new("00000000-0000-0000-C000-000000000046");
    static readonly Guid CallbackIid = typeof(INotificationActivationCallback).GUID;

    [ComImport, Guid("53E31837-6600-4A81-9395-75CFFE746F94"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown), ComVisible(true)]
    public interface INotificationActivationCallback
    {
        void Activate([MarshalAs(UnmanagedType.LPWStr)] string appUserModelId, [MarshalAs(UnmanagedType.LPWStr)] string? invokedArgs, IntPtr data, uint count);
    }

    [ComImport, Guid("00000001-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown), ComVisible(true)]
    public interface IClassFactory
    {
        [PreserveSig] int CreateInstance(IntPtr outer, ref Guid iid, out IntPtr instance);
        [PreserveSig] int LockServer([MarshalAs(UnmanagedType.Bool)] bool lockServer);
    }

    [ComVisible(true), ClassInterface(ClassInterfaceType.None)]
    public sealed class NotificationActivator : INotificationActivationCallback
    {
        public void Activate(string appUserModelId, string? invokedArgs, IntPtr data, uint count)
        {
            if (TryDecode(invokedArgs ?? "", out var activation)) Deliver(activation);
        }
    }

    [ComVisible(true), ClassInterface(ClassInterfaceType.None)]
    public sealed class ActivatorFactory : IClassFactory
    {
        readonly NotificationActivator _activator = new();

        public int CreateInstance(IntPtr outer, ref Guid iid, out IntPtr instance)
        {
            instance = IntPtr.Zero;
            if (outer != IntPtr.Zero) return CLASS_E_NOAGGREGATION;
            if (iid != CallbackIid && iid != IUnknownIid) return E_NOINTERFACE;
            instance = Marshal.GetComInterfaceForObject(_activator, typeof(INotificationActivationCallback));
            return 0;
        }

        public int LockServer(bool lockServer) => 0;
    }

    [DllImport("ole32.dll")]
    static extern int CoRegisterClassObject(ref Guid clsid, [MarshalAs(UnmanagedType.IUnknown)] object factory, uint context, uint flags, out uint cookie);

    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    static extern void SetCurrentProcessExplicitAppUserModelID(string appId);
}
