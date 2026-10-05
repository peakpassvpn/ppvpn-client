using System.Globalization;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Linux.Platform;

/// <summary>
/// The app's own log, next to the crate's: &lt;log_dir&gt;/ppvpn-app.YYYY-MM-DD.log, in the
/// "time LEVEL message" layout the Logs page parses (LogsViewModel).
/// </summary>
public sealed class AppLog(string directory) : IAppLog
{
    private readonly object _gate = new();

    public void Info(string message) => Write("INFO", message);

    public void Warn(string message) => Write("WARN", message);

    public void Error(string message) => Write("ERROR", message);

    private void Write(string level, string message)
    {
        var now = DateTimeOffset.UtcNow;
        var line = $"{now.ToString("yyyy-MM-dd'T'HH:mm:ss.ffffff'Z'", CultureInfo.InvariantCulture)} {level,5} ppvpn_linux: {message}\n";
        lock (_gate)
        {
            try
            {
                Directory.CreateDirectory(directory);
                File.AppendAllText(Path.Combine(directory, $"ppvpn-app.{now:yyyy-MM-dd}.log"), line);
            }
            catch (IOException)
            {
                // Logging must never take the app down.
            }
        }
        if (level != "INFO") Console.Error.Write(line);
    }
}
