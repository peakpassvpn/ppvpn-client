using PPVPN.Linux.App;

// Before GLib starts (see GLibSlices).
PPVPN.Linux.Platform.GLibSlices.UseMalloc();
Adw.Module.Initialize();

// `--background` comes from the XDG autostart entry: start with the tray only.
var background = args.Contains("--background");
var gtkArgs = args.Where(arg => arg != "--background");

#if DEBUG
// `--preview[=<FakeProfileScenario>]` drives the UI with PPVPN.App.Core's FakeClientBackend.
if (args.FirstOrDefault(arg => arg.StartsWith("--preview")) is { } preview)
{
    gtkArgs = gtkArgs.Where(arg => arg != preview);
    var scenario = preview.Contains('=')
        ? Enum.Parse<PPVPN.App.Core.Backend.FakeProfileScenario>(preview.Split('=', 2)[1], ignoreCase: true)
        : PPVPN.App.Core.Backend.FakeProfileScenario.Active;
    var options = new PPVPN.App.Core.Backend.FakeOptions { PersonalTeam = scenario };
    return new PPVPNApplication(background, config => listener =>
    {
        var backend = (PPVPN.App.Core.Backend.FakeClientBackend)PPVPN.App.Core.Backend.FakeClientBackend
            .Factory(config, new PPVPN.Linux.Platform.LinuxPlatformHooks(), options)(listener);
        // Pushes "the agent" showed, for `ppvpn --open-notification <id>` while previewing:
        // 900001 is a broadcast (read-only detail), 900002 is about the second sample message.
        var now = DateTimeOffset.UtcNow.AddMinutes(-3).ToString("yyyy-MM-ddTHH:mm:ssZ", System.Globalization.CultureInfo.InvariantCulture);
        backend.RecordShownPush(new PPVPN.Ffi.PushMessage(900001, null, "今晚香港节点维护",
            "香港节点将于 02:00–02:30（UTC+8）重启，连接会自动切换到其他节点。",
            PPVPN.Ffi.MessageSeverity.Important, PPVPN.Ffi.MessageCategory.Announcement, "announcement.maintenance", null, now));
        backend.RecordShownPush(new PPVPN.Ffi.PushMessage(900002, PPVPN.App.Core.Backend.FakeClientBackend.NewestSampleId - 1,
            "香港 01 线路 HKG-A 不稳定", "HKG-A 在 14:05–14:20 出现间歇性丢包。",
            PPVPN.Ffi.MessageSeverity.Important, PPVPN.Ffi.MessageCategory.Route, "proxy.chain_unhealthy", null, now));
        return backend;
    }).Run(gtkArgs.ToArray());
}
#endif

return new PPVPNApplication(background).Run(gtkArgs.ToArray());
