using System.Runtime.InteropServices;
using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

/// <summary>
/// Smoke test against the real native library (runtimes/&lt;rid&gt;/native from build-dotnet.sh).
/// Signed out with an unreachable API, so it never touches the network or a core.
/// </summary>
public sealed class FfiBackendTests
{
    static bool NativeLibraryPresent()
    {
        var rid = RuntimeInformation.RuntimeIdentifier;
        var name = OperatingSystem.IsWindows() ? "ppvpn_client.dll"
            : OperatingSystem.IsMacOS() ? "libppvpn_client.dylib"
            : "libppvpn_client.so";
        var arch = RuntimeInformation.ProcessArchitecture.ToString().ToLowerInvariant();
        var os = OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux";
        return File.Exists(Path.Combine(AppContext.BaseDirectory, "runtimes", $"{os}-{arch}", "native", name))
            || File.Exists(Path.Combine(AppContext.BaseDirectory, "runtimes", rid, "native", name));
    }

    [Fact]
    public async Task RealClientRestoresToSignedOutThroughTheSynchronizedListener()
    {
        if (!NativeLibraryPresent())
        {
            // No native library for this machine in PPVPN.Client/runtimes; nothing to exercise.
            return;
        }

        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var root = Path.Combine(Path.GetTempPath(), "ppvpn-app-core-tests", Guid.NewGuid().ToString("N"));
            var config = new ClientConfig("http://127.0.0.1:9", Path.Combine(root, "data"), Path.Combine(root, "logs"),
                "linux", "0.0.0-test");
            IClientBackend? backend = null;
            var main = new MainViewModel(
                listener => backend = new FfiClientBackend(config, new MemoryHooks(), listener),
                new TestServices(), new TestSettings(), new KeyLocalizer(), new TestLog(), new TestPrompts());

            await Wait.Until(() => main.Stage == AuthStage.SignedOut, "signed out (no saved credential)");
            // The crate owns the path; only check it is on the API's site root.
            Assert.StartsWith("http://127.0.0.1:9/", main.PurchaseUrl);
            Assert.NotEqual("http://127.0.0.1:9/", main.PurchaseUrl);
            Assert.Equal(AccessState.Ok, main.Access);
            Assert.Equal("notSignedIn", main.StatusLine);
            Assert.Empty(backend!.Nodes());

            await Assert.ThrowsAsync<ClientException.NotSignedIn>(() => backend.Teams());

            await main.ShutdownAsync();
            var (received, offThread, misdelivered) = main.Listener.Stats;
            Assert.True(received > 0);
            Assert.Equal(received, offThread); // the crate calls from its own threads
            Assert.Equal(0, misdelivered);     // and every callback ran on the UI thread
            backend.Dispose();
            try { Directory.Delete(root, recursive: true); } catch (IOException) { }
        });
    }
}
