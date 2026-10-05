using System.Reflection;

namespace PPVPN.Linux.Platform;

/// <summary>
/// Values fixed when the package is built (apps/linux/scripts/build-package.sh passes them as
/// MSBuild properties, stored as AssemblyMetadata), plus the package format the packager wrote
/// next to the app. Nothing here is guessed at run time.
/// </summary>
public static class BuildInfo
{
    private static string? Metadata(string key) =>
        typeof(BuildInfo).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .FirstOrDefault(attribute => attribute.Key == key)?.Value is { Length: > 0 } value ? value : null;

    /// <summary>
    /// Backend for this build's channel: PPVPNApiBase (or the PPVPN_API_BASE environment
    /// variable) at build time, else production.
    /// </summary>
    public static string ApiBase { get; } = Metadata("PPVPNApiBase") ?? "https://www.peakpassvpn.com";

    /// <summary>
    /// "&lt;version&gt;.&lt;release build counter&gt;", as in release-meta and the repository's
    /// latest.json; 0.0.0.0 for local builds, which never see an update notice.
    /// </summary>
    public static Version Build { get; } =
        Version.TryParse($"{typeof(BuildInfo).Assembly.GetName().Version?.ToString(3)}.{Metadata("PPVPNBuildNumber")}", out var build)
            ? build
            : new Version(0, 0, 0, 0);

    /// <summary>latest.json of the apt/dnf repository; null for development builds.</summary>
    public static Uri? UpdateFeed { get; } =
        Uri.TryCreate(Metadata("PPVPNUpdateFeed"), UriKind.Absolute, out var feed) && feed.Scheme == Uri.UriSchemeHttps ? feed : null;

    /// <summary>"deb" or "rpm" from the installed package-format file; null for development builds.</summary>
    public static string? PackageFormat { get; } = ReadPackageFormat();

    private static string? ReadPackageFormat()
    {
        try
        {
            var text = File.ReadAllText(Path.Combine(LinuxPaths.AppDir, "package-format")).Trim();
            return text is "deb" or "rpm" ? text : null;
        }
        catch (IOException)
        {
            return null;
        }
    }
}
