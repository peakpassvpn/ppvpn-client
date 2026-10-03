namespace PPVPN.Linux.Platform;

/// <summary>
/// Per-user directories following the XDG base directory spec. The
/// standard-mode core socket lives in $XDG_RUNTIME_DIR and is chosen by
/// ppvpn-client itself.
/// </summary>
public static class LinuxPaths
{
    private static string Home => Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);

    /// <summary>$XDG_*_HOME when set to an absolute path, else ~/fallback.</summary>
    private static string XdgBase(string variable, string fallback)
    {
        var value = Environment.GetEnvironmentVariable(variable);
        return !string.IsNullOrEmpty(value) && Path.IsPathRooted(value) ? value : Path.Combine(Home, fallback);
    }

    public static string DataDir => Path.Combine(XdgBase("XDG_DATA_HOME", ".local/share"), "ppvpn");

    public static string ConfigDir => Path.Combine(XdgBase("XDG_CONFIG_HOME", ".config"), "ppvpn");

    public static string LogDir => Path.Combine(XdgBase("XDG_STATE_HOME", ".local/state"), "ppvpn", "logs");

    public static string AutostartDir => Path.Combine(XdgBase("XDG_CONFIG_HOME", ".config"), "autostart");

    /// <summary>
    /// Directory of the apphost. The packages install ppvpn-core and the
    /// ppvpn-service helpers next to it in /usr/lib/ppvpn.
    /// </summary>
    public static string AppDir => AppContext.BaseDirectory;
}
