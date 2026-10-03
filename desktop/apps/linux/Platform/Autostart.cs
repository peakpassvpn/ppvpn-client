namespace PPVPN.Linux.Platform;

/// <summary>
/// "Launch at login" through an XDG autostart entry, honoured by GNOME, KDE
/// and most other desktops.
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
                Exec="{Environment.ProcessPath}" --background
                Icon={App.PPVPNApplication.Id}
                X-GNOME-Autostart-enabled=true
                NoDisplay=true

                """);
        }
    }
}
