namespace PPVPN.Linux.Platform;

/// <summary>
/// "Launch at login" through an XDG autostart entry, honoured by GNOME, KDE
/// and most other desktops. Like the menu entry, it caps glibc's malloc
/// arenas: the standard engine runs in this process.
/// </summary>
public static class Autostart
{
    private static string EntryPath => Path.Combine(LinuxPaths.AutostartDir, $"{App.PPVPNApplication.Id}.desktop");

    public static bool Enabled
    {
        get => File.Exists(EntryPath);
        set
        {
            if (!value)
            {
                if (File.Exists(EntryPath)) File.Delete(EntryPath);
                return;
            }
            Directory.CreateDirectory(LinuxPaths.AutostartDir);
            File.WriteAllText(EntryPath, $"""
                [Desktop Entry]
                Type=Application
                Name=PPVPN
                Exec=env MALLOC_ARENA_MAX=2 "{Environment.ProcessPath}" --background
                Icon={App.PPVPNApplication.Id}
                X-GNOME-Autostart-enabled=true
                NoDisplay=true

                """);
        }
    }
}
