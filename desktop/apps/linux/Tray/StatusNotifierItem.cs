using System.Text;
using Tmds.DBus.Protocol;

namespace PPVPN.Linux.Tray;

/// <summary>
/// Tray icon over the StatusNotifierItem protocol (KDE, the GNOME
/// AppIndicator extension, most other panels) with its menu exported as
/// com.canonical.dbusmenu. GTK 4 has no tray API of its own.
///
/// D-Bus calls arrive on thread-pool threads: they only read the immutable
/// <see cref="Layout"/> swapped in by <see cref="Update"/>, and menu
/// activations are posted back to the main thread.
/// </summary>
public sealed class StatusNotifierItem : IDisposable
{
    private const string ItemPath = "/StatusNotifierItem";
    private const string MenuPath = "/MenuBar";
    private const string ItemInterface = "org.kde.StatusNotifierItem";
    private const string MenuInterface = "com.canonical.dbusmenu";
    private const string PropertiesInterface = "org.freedesktop.DBus.Properties";
    private const string WatcherName = "org.kde.StatusNotifierWatcher";

    private readonly DBusConnection _connection;
    private readonly SynchronizationContext _mainThread;
    private readonly string _iconThemePath;
    private readonly Action _activate;
    private readonly string _busName = $"org.kde.StatusNotifierItem-{Environment.ProcessId}-1";
    private volatile Layout _layout;
    private IDisposable? _watcherSubscription;

    /// <summary>
    /// Whether a panel currently shows the icon. Plain GNOME exports the item
    /// fine but has no watcher, so callers must not rely on the tray then.
    /// </summary>
    public bool IsHosted { get; private set; }

    /// <summary>Raised on the main thread when <see cref="IsHosted"/> changes.</summary>
    public event Action? HostedChanged;

    private StatusNotifierItem(
        DBusConnection connection, SynchronizationContext mainThread, string iconThemePath, Action activate, TrayState state)
    {
        _connection = connection;
        _mainThread = mainThread;
        _iconThemePath = iconThemePath;
        _activate = activate;
        _layout = Layout.Build(state, revision: 1);
    }

    /// <summary>
    /// Exports the item and registers it with the panel. Returns null when
    /// there is no session bus; a panel that appears later still picks it up
    /// (see <see cref="IsHosted"/>).
    /// </summary>
    /// <param name="activate">Left click on the icon; runs on the main thread.</param>
    public static async Task<StatusNotifierItem?> StartAsync(
        TrayState state, string iconThemePath, Action activate, SynchronizationContext mainThread)
    {
        if (DBusAddress.Session is null) return null;
        var connection = new DBusConnection(DBusAddress.Session);
        var item = new StatusNotifierItem(connection, mainThread, iconThemePath, activate, state);
        try
        {
            await connection.ConnectAsync();
            connection.AddMethodHandlers([
                new PathHandler(ItemPath, item.HandleItemAsync),
                new PathHandler(MenuPath, item.HandleMenuAsync),
            ]);
            await connection.RequestNameAsync(item._busName, RequestNameOptions.None);
            // Re-register whenever a panel (the watcher) starts or restarts.
            item._watcherSubscription = await connection.WatchSignalAsync(
                "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameOwnerChanged",
                static (message, _) =>
                {
                    var reader = message.GetBodyReader();
                    var name = reader.ReadString();
                    reader.ReadString(); // old owner
                    return (Name: name, HasOwner: reader.ReadString().Length > 0);
                },
                (Notification<(string Name, bool HasOwner)> notification) =>
                {
                    if (!notification.HasValue || notification.Value.Name != WatcherName) return;
                    if (notification.Value.HasOwner) _ = item.RegisterAsync();
                    else item.SetHosted(false);
                },
                ObserverFlags.None, emitOnCapturedContext: false);
            await item.RegisterAsync();
            return item;
        }
        catch (Exception error)
        {
            Console.Error.WriteLine($"tray unavailable: {error.Message}");
            item.Dispose();
            return null;
        }
    }

    private async Task RegisterAsync()
    {
        try
        {
            await _connection.CallMethodAsync(CreateMessage((ref MessageWriter writer) =>
            {
                writer.WriteMethodCallHeader(WatcherName, "/StatusNotifierWatcher", WatcherName,
                    "RegisterStatusNotifierItem", "s", MessageFlags.None);
                writer.WriteString(_busName);
            }));
            SetHosted(true);
        }
        catch (DBusErrorReplyException)
        {
            // No panel with tray support yet; NameOwnerChanged retries later.
            SetHosted(false);
        }
    }

    private void SetHosted(bool hosted)
    {
        void Apply()
        {
            if (IsHosted == hosted) return;
            IsHosted = hosted;
            HostedChanged?.Invoke();
        }
        // Registration during StartAsync resumes on the main thread; apply
        // synchronously there so IsHosted is settled when StartAsync returns.
        if (SynchronizationContext.Current == _mainThread) Apply();
        else _mainThread.Post(_ => Apply(), null);
    }

    /// <summary>Call on the main thread whenever the tray state changes.</summary>
    public void Update(TrayState state)
    {
        var previous = _layout;
        var next = Layout.Build(state, previous.Revision + 1);
        _layout = next;
        if (next.State.IconName != previous.State.IconName) EmitSignal(ItemPath, ItemInterface, "NewIcon");
        if (next.State.Tooltip != previous.State.Tooltip) EmitSignal(ItemPath, ItemInterface, "NewToolTip");

        _connection.TrySendMessage(CreateMessage((ref MessageWriter writer) =>
        {
            writer.WriteSignalHeader(null, MenuPath, MenuInterface, "LayoutUpdated", "ui");
            writer.WriteUInt32(next.Revision);
            writer.WriteInt32(0);
        }));
    }

    public void Dispose()
    {
        _watcherSubscription?.Dispose();
        _connection.Dispose();
    }

    private void EmitSignal(string path, string @interface, string member) =>
        _connection.TrySendMessage(CreateMessage((ref MessageWriter writer) =>
            writer.WriteSignalHeader(null, path, @interface, member, null)));

    // MessageWriter is a ref struct, so bodies are written through a callback.
    private delegate void WriteBody(ref MessageWriter writer);

    private MessageBuffer CreateMessage(WriteBody body)
    {
        var writer = _connection.GetMessageWriter();
        try
        {
            body(ref writer);
            return writer.CreateMessage();
        }
        finally
        {
            writer.Dispose();
        }
    }

    private static void Reply(MethodContext context, string? signature, WriteBody body)
    {
        var writer = context.CreateReplyWriter(signature);
        try
        {
            body(ref writer);
            context.Reply(writer.CreateMessage());
        }
        finally
        {
            writer.Dispose();
        }
    }

    // --- org.kde.StatusNotifierItem ----------------------------------------

    private ValueTask HandleItemAsync(MethodContext context)
    {
        var request = context.Request;
        if (context.IsDBusIntrospectRequest)
        {
            context.ReplyIntrospectXml([ItemIntrospection], ReadOnlySpan<string>.Empty);
            return default;
        }
        switch (request.InterfaceAsString, request.MemberAsString)
        {
            case (PropertiesInterface, _):
                HandleProperties(context, ItemInterface, WriteItemProperty, ItemPropertyNames);
                break;
            case (ItemInterface, "Activate"):
                _mainThread.Post(_ => _activate(), null);
                ReplyEmpty(context);
                break;
            case (ItemInterface, "SecondaryActivate" or "ContextMenu" or "Scroll" or "ProvideXdgActivationToken"):
                ReplyEmpty(context);
                break;
            default:
                context.ReplyUnknownMethodError();
                break;
        }
        return default;
    }

    private static readonly string[] ItemPropertyNames =
    [
        "Category", "Id", "Title", "Status", "IconName", "IconThemePath", "IconPixmap",
        "AttentionIconName", "ToolTip", "ItemIsMenu", "Menu",
    ];

    private void WriteItemProperty(ref MessageWriter writer, string name)
    {
        var state = _layout.State;
        switch (name)
        {
            case "Category": WriteVariantString(ref writer, "ApplicationStatus"); break;
            case "Id": WriteVariantString(ref writer, "ppvpn"); break;
            case "Title": WriteVariantString(ref writer, "PPVPN"); break;
            case "Status": WriteVariantString(ref writer, "Active"); break;
            case "IconName": WriteVariantString(ref writer, state.IconName); break;
            case "AttentionIconName": WriteVariantString(ref writer, ""); break;
            case "IconThemePath": WriteVariantString(ref writer, _iconThemePath); break;
            case "IconPixmap":
                writer.WriteSignature("a(iiay)");
                writer.WriteArrayEnd(writer.WriteArrayStart(DBusType.Struct));
                break;
            case "ToolTip":
                writer.WriteSignature("(sa(iiay)ss)");
                writer.WriteStructureStart();
                writer.WriteString("");
                writer.WriteArrayEnd(writer.WriteArrayStart(DBusType.Struct));
                writer.WriteString("PPVPN");
                writer.WriteString(state.Tooltip);
                break;
            case "ItemIsMenu":
                writer.WriteSignature("b");
                writer.WriteBool(true);
                break;
            case "Menu":
                writer.WriteSignature("o");
                writer.WriteObjectPath(MenuPath);
                break;
        }
    }

    // --- com.canonical.dbusmenu --------------------------------------------

    private ValueTask HandleMenuAsync(MethodContext context)
    {
        var request = context.Request;
        if (context.IsDBusIntrospectRequest)
        {
            context.ReplyIntrospectXml([MenuIntrospection], ReadOnlySpan<string>.Empty);
            return default;
        }
        var layout = _layout;
        switch (request.InterfaceAsString, request.MemberAsString)
        {
            case (PropertiesInterface, _):
                HandleProperties(context, MenuInterface, WriteMenuProperty, MenuPropertyNames);
                break;
            case (MenuInterface, "GetLayout"):
            {
                var reader = request.GetBodyReader();
                var parentId = reader.ReadInt32();
                var depth = reader.ReadInt32();
                if (!layout.Nodes.TryGetValue(parentId, out var parent))
                {
                    context.ReplyError($"{MenuInterface}.Error", $"unknown menu item {parentId}");
                    break;
                }
                Reply(context, "u(ia{sv}av)", (ref MessageWriter writer) =>
                {
                    writer.WriteUInt32(layout.Revision);
                    WriteLayoutNode(ref writer, layout, parent, depth);
                });
                break;
            }
            case (MenuInterface, "GetGroupProperties"):
            {
                var ids = request.GetBodyReader().ReadArrayOfInt32();
                Reply(context, "a(ia{sv})", (ref MessageWriter writer) =>
                {
                    var array = writer.WriteArrayStart(DBusType.Struct);
                    foreach (var id in ids.Length > 0 ? ids : layout.Nodes.Keys.ToArray())
                    {
                        if (!layout.Nodes.TryGetValue(id, out var node)) continue;
                        writer.WriteStructureStart();
                        writer.WriteInt32(id);
                        WriteNodeProperties(ref writer, node);
                    }
                    writer.WriteArrayEnd(array);
                });
                break;
            }
            case (MenuInterface, "Event"):
            {
                var reader = request.GetBodyReader();
                Dispatch(layout, reader.ReadInt32(), reader.ReadString());
                ReplyEmpty(context);
                break;
            }
            case (MenuInterface, "EventGroup"):
            {
                var reader = request.GetBodyReader();
                var end = reader.ReadArrayStart(DBusType.Struct);
                while (reader.HasNext(end))
                {
                    reader.AlignStruct();
                    var id = reader.ReadInt32();
                    var eventId = reader.ReadString();
                    reader.ReadVariantValue();
                    reader.ReadUInt32();
                    Dispatch(layout, id, eventId);
                }
                Reply(context, "ai", (ref MessageWriter writer) => writer.WriteArray(Array.Empty<int>()));
                break;
            }
            case (MenuInterface, "AboutToShow"):
            {
                Reply(context, "b", (ref MessageWriter writer) => writer.WriteBool(false));
                break;
            }
            case (MenuInterface, "AboutToShowGroup"):
            {
                Reply(context, "aiai", (ref MessageWriter writer) =>
                {
                    writer.WriteArray(Array.Empty<int>());
                    writer.WriteArray(Array.Empty<int>());
                });
                break;
            }
            default:
                context.ReplyUnknownMethodError();
                break;
        }
        return default;
    }

    private void Dispatch(Layout layout, int id, string eventId)
    {
        if (eventId != "clicked" || !layout.Nodes.TryGetValue(id, out var node)) return;
        if (node.Item?.Activated is { } action && node.Item.Enabled) _mainThread.Post(_ => action(), null);
    }

    private static readonly string[] MenuPropertyNames = ["Version", "TextDirection", "Status", "IconThemePath"];

    private void WriteMenuProperty(ref MessageWriter writer, string name)
    {
        switch (name)
        {
            case "Version":
                writer.WriteSignature("u");
                writer.WriteUInt32(3);
                break;
            case "TextDirection": WriteVariantString(ref writer, "ltr"); break;
            case "Status": WriteVariantString(ref writer, "normal"); break;
            case "IconThemePath":
                writer.WriteSignature("as");
                writer.WriteArray(new[] { _iconThemePath });
                break;
        }
    }

    private static void WriteLayoutNode(ref MessageWriter writer, Layout layout, Layout.Node node, int depth)
    {
        writer.WriteStructureStart();
        writer.WriteInt32(node.Id);
        WriteNodeProperties(ref writer, node);
        var children = writer.WriteArrayStart(DBusType.Variant);
        if (depth != 0)
        {
            foreach (var childId in node.ChildIds)
            {
                writer.WriteSignature("(ia{sv}av)");
                WriteLayoutNode(ref writer, layout, layout.Nodes[childId], depth - 1);
            }
        }
        writer.WriteArrayEnd(children);
    }

    private static void WriteNodeProperties(ref MessageWriter writer, Layout.Node node)
    {
        var dictionary = writer.WriteDictionaryStart();
        void Entry(ref MessageWriter w, string key)
        {
            w.WriteDictionaryEntryStart();
            w.WriteString(key);
        }

        if (node.Item is not { } item)
        {
            Entry(ref writer, "children-display");
            WriteVariantString(ref writer, "submenu");
        }
        else if (item.IsSeparator)
        {
            Entry(ref writer, "type");
            WriteVariantString(ref writer, "separator");
        }
        else
        {
            Entry(ref writer, "label");
            // dbusmenu treats a single underscore as a mnemonic marker.
            WriteVariantString(ref writer, item.Label.Replace("_", "__"));
            Entry(ref writer, "enabled");
            writer.WriteSignature("b");
            writer.WriteBool(item.Enabled);
            if (item.Toggle != TrayToggle.None)
            {
                Entry(ref writer, "toggle-type");
                WriteVariantString(ref writer, item.Toggle == TrayToggle.Radio ? "radio" : "checkmark");
                Entry(ref writer, "toggle-state");
                writer.WriteSignature("i");
                writer.WriteInt32(item.Checked ? 1 : 0);
            }
            if (item.Children.Count > 0)
            {
                Entry(ref writer, "children-display");
                WriteVariantString(ref writer, "submenu");
            }
        }
        writer.WriteDictionaryEnd(dictionary);
    }

    // --- org.freedesktop.DBus.Properties -----------------------------------

    private delegate void PropertyWriter(ref MessageWriter writer, string name);

    private static void HandleProperties(MethodContext context, string @interface, PropertyWriter write, string[] names)
    {
        var request = context.Request;
        var reader = request.GetBodyReader();
        switch (request.MemberAsString)
        {
            case "Get":
            {
                var requestedInterface = reader.ReadString();
                var name = reader.ReadString();
                if (requestedInterface != @interface || !names.Contains(name))
                {
                    context.ReplyError("org.freedesktop.DBus.Error.UnknownProperty", $"{requestedInterface}.{name}");
                    return;
                }
                Reply(context, "v", (ref MessageWriter writer) => write(ref writer, name));
                return;
            }
            case "GetAll":
            {
                var requestedInterface = reader.ReadString();
                Reply(context, "a{sv}", (ref MessageWriter writer) =>
                {
                    var dictionary = writer.WriteDictionaryStart();
                    if (requestedInterface == @interface)
                    {
                        foreach (var name in names)
                        {
                            writer.WriteDictionaryEntryStart();
                            writer.WriteString(name);
                            write(ref writer, name);
                        }
                    }
                    writer.WriteDictionaryEnd(dictionary);
                });
                return;
            }
            case "Set":
                context.ReplyError("org.freedesktop.DBus.Error.PropertyReadOnly", "read-only");
                return;
            default:
                context.ReplyUnknownMethodError();
                return;
        }
    }

    private static void WriteVariantString(ref MessageWriter writer, string value)
    {
        writer.WriteSignature("s");
        writer.WriteString(value);
    }

    private static void ReplyEmpty(MethodContext context)
    {
        if (!context.NoReplyExpected) Reply(context, null, (ref MessageWriter _) => { });
    }

    private static readonly ReadOnlyMemory<byte> ItemIntrospection = Encoding.UTF8.GetBytes("""
        <interface name="org.kde.StatusNotifierItem">
          <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
          <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
          <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
          <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
          <signal name="NewIcon"/>
          <signal name="NewToolTip"/>
          <property name="Category" type="s" access="read"/>
          <property name="Id" type="s" access="read"/>
          <property name="Title" type="s" access="read"/>
          <property name="Status" type="s" access="read"/>
          <property name="IconName" type="s" access="read"/>
          <property name="IconThemePath" type="s" access="read"/>
          <property name="IconPixmap" type="a(iiay)" access="read"/>
          <property name="AttentionIconName" type="s" access="read"/>
          <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
          <property name="ItemIsMenu" type="b" access="read"/>
          <property name="Menu" type="o" access="read"/>
        </interface>
        """);

    private static readonly ReadOnlyMemory<byte> MenuIntrospection = Encoding.UTF8.GetBytes("""
        <interface name="com.canonical.dbusmenu">
          <method name="GetLayout">
            <arg name="parentId" type="i" direction="in"/><arg name="recursionDepth" type="i" direction="in"/>
            <arg name="propertyNames" type="as" direction="in"/>
            <arg name="revision" type="u" direction="out"/><arg name="layout" type="(ia{sv}av)" direction="out"/>
          </method>
          <method name="GetGroupProperties">
            <arg name="ids" type="ai" direction="in"/><arg name="propertyNames" type="as" direction="in"/>
            <arg name="properties" type="a(ia{sv})" direction="out"/>
          </method>
          <method name="Event">
            <arg name="id" type="i" direction="in"/><arg name="eventId" type="s" direction="in"/>
            <arg name="data" type="v" direction="in"/><arg name="timestamp" type="u" direction="in"/>
          </method>
          <method name="EventGroup">
            <arg name="events" type="a(isvu)" direction="in"/><arg name="idErrors" type="ai" direction="out"/>
          </method>
          <method name="AboutToShow">
            <arg name="id" type="i" direction="in"/><arg name="needUpdate" type="b" direction="out"/>
          </method>
          <method name="AboutToShowGroup">
            <arg name="ids" type="ai" direction="in"/>
            <arg name="updatesNeeded" type="ai" direction="out"/><arg name="idErrors" type="ai" direction="out"/>
          </method>
          <signal name="LayoutUpdated"><arg name="revision" type="u"/><arg name="parent" type="i"/></signal>
          <property name="Version" type="u" access="read"/>
          <property name="TextDirection" type="s" access="read"/>
          <property name="Status" type="s" access="read"/>
          <property name="IconThemePath" type="as" access="read"/>
        </interface>
        """);

    private sealed class PathHandler(string path, Func<MethodContext, ValueTask> handle) : IPathMethodHandler
    {
        public string Path => path;
        public bool HandlesChildPaths => false;
        public ValueTask HandleMethodAsync(MethodContext context) => handle(context);
    }

    /// <summary>The menu tree flattened into dbusmenu ids; id 0 is the root.</summary>
    private sealed class Layout
    {
        public sealed record Node(int Id, TrayMenuItem? Item, int[] ChildIds);

        public required uint Revision { get; init; }
        public required TrayState State { get; init; }
        public required IReadOnlyDictionary<int, Node> Nodes { get; init; }

        public static Layout Build(TrayState state, uint revision)
        {
            var nodes = new Dictionary<int, Node>();
            var nextId = 1;
            int[] Add(IReadOnlyList<TrayMenuItem> items) => items.Select(item =>
            {
                var id = nextId++;
                nodes[id] = new Node(id, item, Add(item.Children));
                return id;
            }).ToArray();
            nodes[0] = new Node(0, null, Add(state.Menu));
            return new Layout { Revision = revision, State = state, Nodes = nodes };
        }
    }
}
