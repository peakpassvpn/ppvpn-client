using System.Globalization;
using System.Text;

namespace PPVPN.PushAgent;

/// <summary>
/// The agent's own log, <c>%LOCALAPPDATA%\PPVPN\logs\ppvpn-push-agent-windows.YYYY-MM-DD.log</c> (UTC
/// date, like the app's <c>ppvpn-windows.*</c>). The crate writes <c>ppvpn-push-agent.*</c> next to it
/// (pull, ack, state); this file has the Windows side: process lifetime, toasts, the setting.
/// Files older than 7 days are deleted at startup.
/// </summary>
sealed class AgentLog
{
    const string Prefix = "ppvpn-push-agent-windows.";
    const int KeepDays = 7;

    readonly string _dir;
    readonly object _gate = new();

    public AgentLog(string dir)
    {
        _dir = dir;
        try
        {
            Directory.CreateDirectory(dir);
            Prune();
        }
        catch (Exception) { }
    }

    public void Info(string message) => Write("INFO", message);

    public void Warn(string message) => Write("WARN", message);

    public void Error(string message) => Write("ERROR", message);

    void Write(string level, string message)
    {
        var now = DateTimeOffset.Now;
        var line = $"{now.ToString("yyyy-MM-ddTHH:mm:ss.fffzzz", CultureInfo.InvariantCulture)} {level} ppvpn_push_agent_windows: {message}{Environment.NewLine}";
        lock (_gate)
        {
            try
            {
                var path = Path.Combine(_dir, $"{Prefix}{now.UtcDateTime.ToString("yyyy-MM-dd", CultureInfo.InvariantCulture)}.log");
                using var stream = new FileStream(path, FileMode.Append, FileAccess.Write, FileShare.ReadWrite | FileShare.Delete);
                var bytes = Encoding.UTF8.GetBytes(line);
                stream.Write(bytes, 0, bytes.Length);
            }
            catch (Exception) { }
        }
    }

    void Prune()
    {
        var oldest = DateTime.UtcNow.Date.AddDays(-(KeepDays - 1));
        foreach (var file in Directory.EnumerateFiles(_dir, Prefix + "*.log"))
        {
            var date = Path.GetFileNameWithoutExtension(file)[Prefix.Length..];
            if (DateTime.TryParseExact(date, "yyyy-MM-dd", CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal | DateTimeStyles.AdjustToUniversal, out var day) && day < oldest)
            {
                try { File.Delete(file); } catch (Exception) { }
            }
        }
    }
}
