using static PPVPN.Windows.Platform.NativeMethods;

namespace PPVPN.Windows.Platform;

public static class Shell
{
    /// <summary>
    /// Open an http(s) URL in the default browser. Anything else is refused so
    /// a hostile backend response can never launch a local program.
    /// Safe to call from any thread.
    /// </summary>
    public static bool OpenUrl(string url)
    {
        if (!Uri.TryCreate(url, UriKind.Absolute, out var uri)
            || (uri.Scheme != Uri.UriSchemeHttps && uri.Scheme != Uri.UriSchemeHttp))
            return false;
        return RunOnStaThread(() => (long)ShellExecute(IntPtr.Zero, "open", uri.AbsoluteUri, null, null, SW_SHOWNORMAL) > 32);
    }

    /// <summary>Open a Windows Settings page (<c>ms-settings:</c> URIs only).</summary>
    public static bool OpenSettingsPage(string uri)
    {
        if (!uri.StartsWith("ms-settings:", StringComparison.OrdinalIgnoreCase)) return false;
        return RunOnStaThread(() => (long)ShellExecute(IntPtr.Zero, "open", uri, null, null, SW_SHOWNORMAL) > 32);
    }

    /// <summary>Show a file selected in File Explorer (its folder when the file does not exist).</summary>
    public static bool RevealFile(string path)
    {
        if (!File.Exists(path)) return OpenFolder(Path.GetDirectoryName(path) ?? path);
        return RunOnStaThread(() => (long)ShellExecute(IntPtr.Zero, "open", "explorer.exe", $"/select,\"{path}\"", null, SW_SHOWNORMAL) > 32);
    }

    /// <summary>Open a folder in File Explorer.</summary>
    public static bool OpenFolder(string path)
    {
        if (!Directory.Exists(path)) return false;
        return RunOnStaThread(() => (long)ShellExecute(IntPtr.Zero, "open", path, null, null, SW_SHOWNORMAL) > 32);
    }

    /// <summary>ShellExecute wants an STA; callbacks from Rust arrive on MTA threads.</summary>
    static bool RunOnStaThread(Func<bool> action)
    {
        if (Thread.CurrentThread.GetApartmentState() == ApartmentState.STA) return action();
        var result = false;
        var thread = new Thread(() => result = action()) { IsBackground = true, Name = "ppvpn-shell" };
        thread.SetApartmentState(ApartmentState.STA);
        thread.Start();
        thread.Join();
        return result;
    }
}
