// Hand-written (not generated). Resolves the "ppvpn_client" native library that
// the UniFFI bindings import from runtimes/<rid>/native/ next to the app: with a
// ProjectReference (unlike a NuGet package) .NET does not probe that folder.
using System;
using System.Diagnostics.CodeAnalysis;
using System.IO;
using System.Reflection;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;

namespace PPVPN.Ffi;

public static class NativeLoader
{
    public const string LibraryName = "ppvpn_client";

    private static readonly object Gate = new();
    private static bool _installed;

    /// Path the native library was loaded from by this resolver, or null when it
    /// has not been loaded yet or was found by the runtime's default probing.
    public static string? LoadedFrom { get; private set; }

    /// Registers the resolver for the PPVPN.Client assembly. Idempotent. It also
    /// runs automatically when the assembly loads; apps may call it explicitly
    /// (once, before any Client use) to make the dependency obvious.
    public static void Install()
    {
        lock (Gate)
        {
            if (_installed)
            {
                return;
            }
            NativeLibrary.SetDllImportResolver(typeof(NativeLoader).Assembly, Resolve);
            _installed = true;
        }
    }

    [ModuleInitializer]
    internal static void InstallOnLoad() => Install();

    private static IntPtr Resolve(string name, Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (name != LibraryName)
        {
            return IntPtr.Zero;
        }

        var file = OperatingSystem.IsWindows() ? LibraryName + ".dll"
                 : OperatingSystem.IsMacOS() ? "lib" + LibraryName + ".dylib"
                 : "lib" + LibraryName + ".so";
        var os = OperatingSystem.IsWindows() ? "win" : OperatingSystem.IsMacOS() ? "osx" : "linux";
        var portableRid = os + "-" + RuntimeInformation.ProcessArchitecture.ToString().ToLowerInvariant();

        // Assembly.Location is always empty under NativeAOT (the push agent); the
        // app base directory covers that case.
        var baseDirs = RuntimeFeature.IsDynamicCodeSupported
            ? new[] { AppContext.BaseDirectory, AssemblyDirectory(assembly) }
            : new[] { AppContext.BaseDirectory };
        foreach (var baseDir in baseDirs)
        {
            if (string.IsNullOrEmpty(baseDir))
            {
                continue;
            }
            foreach (var rid in new[] { RuntimeInformation.RuntimeIdentifier, portableRid })
            {
                var candidate = Path.Combine(baseDir, "runtimes", rid, "native", file);
                if (File.Exists(candidate))
                {
                    // Load (not TryLoad) so a present-but-unloadable library surfaces the
                    // loader's reason (e.g. "GLIBC_2.39 not found") instead of a generic
                    // DllNotFoundException from the default probing.
                    var handle = NativeLibrary.Load(candidate);
                    LoadedFrom = candidate;
                    return handle;
                }
            }
        }

        // Fall back to the runtime's default probing (app dir, deps.json, system paths).
        return IntPtr.Zero;
    }

    // Only called when dynamic code is supported, i.e. never under NativeAOT.
    [UnconditionalSuppressMessage("SingleFile", "IL3000", Justification = "Not reached under NativeAOT or single-file.")]
    private static string? AssemblyDirectory(Assembly assembly) => Path.GetDirectoryName(assembly.Location);
}
