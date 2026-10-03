using System.Net;
using System.Net.Sockets;
using System.Text;
using PPVPN.App.Core.ViewModels;
using PPVPN.Linux.Update;

namespace PPVPN.Linux.Tests;

public sealed class UpdateNotifierTests : IDisposable
{
    private static readonly Version Installed = new(0, 3, 0, 12);
    private readonly LatestServer _server = new();

    public UpdateNotifierTests() => PPVPN.Linux.UI.L.Initialize(new JsonLocalizer());

    public void Dispose() => _server.Dispose();

    private UpdateNotifier Notifier(string format = "deb") =>
        new(new Uri(_server.Url), Installed, format, new NullLog(), "0.3.0");

    [Fact]
    public async Task Newer_build_shows_the_banner_with_the_apt_command()
    {
        _server.Body = """{"version":"0.3.1","build":"0.3.1.15","published_at":"2026-09-29T12:00:00Z"}""";
        var notifier = Notifier();
        await notifier.CheckAsync(userInitiated: false);
        Assert.True(notifier.IsAvailable);
        Assert.True(notifier.IsBannerVisible);
        Assert.Equal("0.3.1", notifier.AvailableVersion);
        Assert.Equal("sudo apt update && sudo apt install --only-upgrade ppvpn", notifier.Command);
        Assert.Equal("sudo dnf upgrade ppvpn", Notifier("rpm").Command);
    }

    [Fact]
    public async Task A_newer_build_of_the_same_version_shows_its_build_number()
    {
        _server.Body = """{"version":"0.3.0","build":"0.3.0.13"}""";
        var notifier = Notifier();
        string? toast = null;
        notifier.CheckFinished += text => toast = text;
        await notifier.CheckAsync(userInitiated: true);
        Assert.True(notifier.IsAvailable);
        Assert.Equal("0.3.0 (13)", notifier.AvailableVersion);
        Assert.Contains("0.3.0 (13)", toast);
    }

    [Theory]
    [InlineData("0.3.0.12")]
    [InlineData("0.2.9.40")]
    public async Task Same_or_older_build_is_up_to_date(string build)
    {
        _server.Body = $$"""{"version":"0.3.0","build":"{{build}}"}""";
        var notifier = Notifier();
        string? toast = null;
        notifier.CheckFinished += text => toast = text;
        await notifier.CheckAsync(userInitiated: true);
        Assert.False(notifier.IsAvailable);
        Assert.False(notifier.IsBannerVisible);
        Assert.NotNull(toast);
    }

    [Fact]
    public async Task Later_hides_the_banner_until_a_newer_build()
    {
        _server.Body = """{"version":"0.3.1","build":"0.3.1.15"}""";
        var notifier = Notifier();
        await notifier.CheckAsync(userInitiated: false);
        notifier.Later();
        await notifier.CheckAsync(userInitiated: false);
        Assert.False(notifier.IsBannerVisible);
        notifier.ShowBanner();
        Assert.True(notifier.IsBannerVisible);

        notifier.Later();
        _server.Body = """{"version":"0.3.2","build":"0.3.2.16"}""";
        await notifier.CheckAsync(userInitiated: false);
        Assert.True(notifier.IsBannerVisible);
        Assert.Equal("0.3.2", notifier.AvailableVersion);
    }

    [Theory]
    [InlineData("not json")]
    [InlineData("""{"version":"0.3.1"}""")]
    [InlineData("""{"version":"0.3.1","build":"latest"}""")]
    public async Task Bad_feed_is_a_failed_check(string body)
    {
        _server.Body = body;
        var notifier = Notifier();
        string? toast = null;
        notifier.CheckFinished += text => toast = text;
        await notifier.CheckAsync(userInitiated: true);
        Assert.False(notifier.IsAvailable);
        Assert.NotNull(toast);
    }

    [Fact]
    public void Development_builds_do_not_check()
    {
        var feed = new Uri("https://pkg.peakpassvpn.com/linux/latest.json");
        Assert.False(new UpdateNotifier(null, Installed, "deb", new NullLog(), "0.3.0").CanCheck);
        Assert.False(new UpdateNotifier(feed, Installed, null, new NullLog(), "0.3.0").CanCheck);
        Assert.False(new UpdateNotifier(feed, new Version(0, 0, 0, 0), "deb", new NullLog(), "0.3.0").CanCheck);
        Assert.True(new UpdateNotifier(feed, Installed, "rpm", new NullLog(), "0.3.0").CanCheck);
    }

    private sealed class NullLog : IAppLog
    {
        public void Info(string message) { }
        public void Warn(string message) { }
        public void Error(string message) { }
    }

    /// <summary>Serves latest.json over loopback HTTP.</summary>
    private sealed class LatestServer : IDisposable
    {
        private readonly HttpListener _listener = new();

        public LatestServer()
        {
            using var probe = new TcpListener(IPAddress.Loopback, 0);
            probe.Start();
            var port = ((IPEndPoint)probe.LocalEndpoint).Port;
            probe.Stop();
            Url = $"http://127.0.0.1:{port}/linux/latest.json";
            _listener.Prefixes.Add($"http://127.0.0.1:{port}/");
            _listener.Start();
            _ = Task.Run(ServeAsync);
        }

        public string Url { get; }
        public string Body { get; set; } = "{}";

        private async Task ServeAsync()
        {
            while (_listener.IsListening)
            {
                HttpListenerContext context;
                try
                {
                    context = await _listener.GetContextAsync();
                }
                catch (Exception)
                {
                    return;
                }
                context.Response.ContentType = "application/json";
                await context.Response.OutputStream.WriteAsync(Encoding.UTF8.GetBytes(Body));
                context.Response.Close();
            }
        }

        public void Dispose() => _listener.Close();
    }
}
