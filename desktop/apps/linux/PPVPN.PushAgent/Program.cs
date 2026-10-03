using System.Runtime.InteropServices;
using PPVPN.Ffi;
using PPVPN.Linux.Platform;
using PPVPN.PushAgent;

// One agent per user. The lock (flock, taken by FileShare.None) goes with the process, so a
// crashed agent never blocks the next one.
var lockPath = Path.Combine(RuntimeDir() ?? LinuxPaths.DataDir, "ppvpn-push-agent.lock");
FileStream instanceLock;
try
{
    Directory.CreateDirectory(Path.GetDirectoryName(lockPath)!);
    instanceLock = new FileStream(lockPath, FileMode.OpenOrCreate, FileAccess.ReadWrite, FileShare.None);
}
catch (IOException)
{
    return 0; // already running
}

using var notifier = new DesktopNotifier(Path.Combine(LinuxPaths.AppDir, "ppvpn"));
// Waited for synchronously on purpose: after an await, this method would continue on the
// D-Bus reader thread (replies complete inline), and agent.Run() below would block it for good.
if (!notifier.ConnectAsync().GetAwaiter().GetResult())
{
    Console.Error.WriteLine("ppvpn-push-agent: no D-Bus session bus");
    return 1;
}

var version = typeof(DesktopNotifier).Assembly.GetName().Version!;
var config = new PPVPN.Ffi.PushAgentConfig(
    DataDir: LinuxPaths.DataDir,
    LogDir: LinuxPaths.LogDir,
    Platform: "linux",
    AppVersion: $"{version.Major}.{version.Minor}.{version.Build}");
Directory.CreateDirectory(config.LogDir);
using var agent = new PPVPN.Ffi.PushAgent(config, new Listener(notifier));

// Pushes that found no notification server are offered again as soon as one appears.
notifier.ServerAppeared += agent.RetryPending;
// The session bus goes away with the session: nothing left to show notifications on.
notifier.Disconnected += agent.Stop;
void Stop(PosixSignalContext context)
{
    context.Cancel = true;
    agent.Stop();
}
using var term = PosixSignalRegistration.Create(PosixSignal.SIGTERM, Stop);
using var interrupt = PosixSignalRegistration.Create(PosixSignal.SIGINT, Stop);
using var hangup = PosixSignalRegistration.Create(PosixSignal.SIGHUP, Stop);

agent.Run();
instanceLock.Dispose();
return 0;

static string? RuntimeDir()
{
    var dir = Environment.GetEnvironmentVariable("XDG_RUNTIME_DIR");
    return !string.IsNullOrEmpty(dir) && Path.IsPathRooted(dir) ? dir : null;
}

namespace PPVPN.PushAgent
{
    /// <summary>Called by the Rust agent on its own threads.</summary>
    internal sealed class Listener(DesktopNotifier notifier) : PushAgentListener
    {
        public bool OnPush(PushMessage message) => notifier.Show(message);

        // The state is in the agent's log; nothing to show for it.
        public void OnState(PushAgentState state) { }
    }
}
