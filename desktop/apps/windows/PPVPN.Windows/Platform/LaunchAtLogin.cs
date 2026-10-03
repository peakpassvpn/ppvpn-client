using Microsoft.Win32;

namespace PPVPN.Windows.Platform;

/// <summary>Per-user autostart through HKCU\...\Run.</summary>
public static class LaunchAtLogin
{
    const string RunKey = @"Software\Microsoft\Windows\CurrentVersion\Run";
    const string ValueName = "PPVPN";

    /// <summary>Command-line flag that starts the app hidden in the tray.</summary>
    public const string BackgroundArgument = "--background";

    static string Command => $"\"{Environment.ProcessPath}\" {BackgroundArgument}";

    public static bool IsEnabled
    {
        get
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKey);
            return key?.GetValue(ValueName) is string value && value.Length > 0;
        }
    }

    /// <exception cref="UnauthorizedAccessException"></exception>
    /// <exception cref="System.Security.SecurityException"></exception>
    public static void Set(bool enabled)
    {
        using var key = Registry.CurrentUser.CreateSubKey(RunKey, writable: true);
        if (enabled) key.SetValue(ValueName, Command, RegistryValueKind.String);
        else key.DeleteValue(ValueName, throwOnMissingValue: false);
    }

    const string ApprovedKey = @"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    /// <summary>
    /// The Run entry exists but was turned off in Settings › Apps › Startup (or Task Manager):
    /// Explorer records that in StartupApproved\Run as a binary value whose first byte is odd
    /// (02/06 = enabled, 03/07 = disabled). Only Windows can turn it back on.
    /// </summary>
    public static bool IsBlockedBySystem
    {
        get
        {
            try
            {
                if (!IsEnabled) return false;
                using var key = Registry.CurrentUser.OpenSubKey(ApprovedKey);
                return key?.GetValue(ValueName) is byte[] { Length: > 0 } value && (value[0] & 1) == 1;
            }
            catch (Exception error) when (error is UnauthorizedAccessException or System.Security.SecurityException or IOException)
            {
                return false;
            }
        }
    }

    /// <summary>Settings › Apps › Startup.</summary>
    public static void OpenSystemSettings() => Shell.OpenSettingsPage("ms-settings:startupapps");

    /// <summary>Point an existing entry at the current exe (the app may have moved).</summary>
    public static void Refresh()
    {
        try
        {
            using var key = Registry.CurrentUser.OpenSubKey(RunKey, writable: true);
            if (key?.GetValue(ValueName) is string value && value != Command) key.SetValue(ValueName, Command);
        }
        catch (UnauthorizedAccessException) { }
    }
}
