using System.Runtime.InteropServices;

namespace PPVPN.Linux.Platform;

/// <summary>
/// Makes GLib's slice allocator plain malloc. Gir.Core hands GLib some boxed records it
/// allocated itself (e.g. the <c>Gtk.TextIter</c> of <c>GetEndIter</c>), which GLib then frees
/// with <c>g_slice_free</c>. GLib before 2.76 (Ubuntu 22.04 has 2.72) has a real slice
/// allocator, and the mismatch corrupts it: "GSlice: assertion failed: sinfo->n_allocated > 0",
/// then SIGSEGV or SIGABRT, e.g. after a while on the Logs page. Later GLibs always use malloc.
/// </summary>
public static class GLibSlices
{
    /// <summary>
    /// Sets <c>G_SLICE=always-malloc</c> in the process environment, unless already set. Call it
    /// before anything starts GLib; .NET's own environment API does not reach the C environment.
    /// </summary>
    public static void UseMalloc() => setenv("G_SLICE", "always-malloc", 0);

    [DllImport("libc", SetLastError = true)]
    private static extern int setenv(string name, string value, int overwrite);
}
