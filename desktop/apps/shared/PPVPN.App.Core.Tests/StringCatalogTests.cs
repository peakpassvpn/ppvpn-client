using System.Globalization;
using System.Text.Json;
using System.Text.RegularExpressions;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

public class StringCatalogTests
{
    static readonly string[] Languages = [JsonLocalizer.Chinese, JsonLocalizer.English];

    /// <summary>Keys the design handoff's strings.json defines (the catalog is a superset).</summary>
    static IReadOnlyList<string> DesignKeys()
    {
        var path = Path.Combine(AppContext.BaseDirectory, "Fixtures", "design-strings.json");
        using var stream = File.OpenRead(path);
        var design = JsonSerializer.Deserialize<Dictionary<string, Dictionary<string, string>>>(stream)!;
        return design["zh"].Keys.ToList();
    }

    /// <summary>Every key the view models look up (README "Localisation keys").</summary>
    static IEnumerable<string> RequiredKeys() =>
        ErrorMessages.AllKeys
            .Concat(ErrorMessages.OtherKeys)
            .Concat(Enum.GetValues<ConnectState>().Select(s => $"st_{s.ToString().ToLowerInvariant()}"))
            .Concat(Enum.GetValues<MessageCategory>().Select(MessageTypes.LabelKey))
            .Concat([
                "std_on", "std_off", "std_starting", "std_failed",
                "h_on", "h_failed", "h_occupied", "h_idle", "h_stdStarting", "h_stdFail", "h_pathContended", "conflictT", "conflictD",
                "d_preparing", "d_authorizing", "d_connecting", "d_on", "d_reconnecting", "d_occupied", "d_disconnecting", "d_idle", "d_idleProxyFailed", "d_stdOn",
                "fr_timeout", "fr_coreStopped", "fr_auth", "fr_port", "tunMode", "stdMode", "tunDesc", "stdDesc", "retry", "takeOver", "useCompatible",
                "connect", "captionEnhanced", "captionCompatible", "connMethod", "methodEnhanced", "methodEnhancedD", "methodCompatible", "methodCompatibleD",
                "routingMode", "routingRules", "routingRulesD", "routingGlobal", "routingGlobalD",
                "expiresIn", "errExpiredT", "errExpiredD", "errDeniedT", "errDeniedD", "errNetT", "errNetD", "signInAgain", "tryAgain", "errorT", "errLockedT",
                "noSubT", "noSubD", "expiredT", "expiredD", "teamOffT", "teamOffD", "expiredOn", "serviceExpiresOn", "personal", "disabled",
                "nodeCount", "nodeProxyT", "routeN", "testingWith", "timeout", "failed", "proxyStarting", "proxyRouted", "proxyNoteRouted", "editRoutingRules", "editRoutingRulesD", "proxyRoutedD", "proxyNode", "proxyNodeD", "proxyFailed", "proxyFailedT",
                "rulesT", "rulesUnavailableD", "lineAuto", "d_switched", "ingressDownT", "ingressDownD", "backToAuto", "pinClearedT", "pinClearedD",
                "client", "core", "today", "yesterday",
                "logAllLevels", "logDebugUp", "logInfoUp", "logWarnUp", "logErrorOnly", "logSearch", "logNoMatches", "copyShownLogs",
                "notSignedIn", "openMain", "settingsMenuWin", "preferences", "checkUpdates", "quit", "unreadN", "noUnread", "currentNodeIs",
                "unreadShort", "allRead", "sev_critical", "sev_important", "justNow", "minAgo", "timeToday", "timeYesterday", "markRead", "markUnread",
                "installT", "installD", "installGo", "cancel", "uninstallQ", "uninstallD", "uninstallConfirm", "signOutQ", "signOutD", "signOut",
                "switchFailT", "switchFailD", "installFailT", "uninstallFailT", "launchFailT", "ok",
                "serviceOn", "serviceOff", "version", "checkNow", "nodesHint",
            ]);

    /// <summary>Keys that name a platform on purpose (each platform picks its own).</summary>
    static readonly HashSet<string> PlatformKeys =
    [
        "revealMac", "revealWin", "revealLinux", "settingsMenuMac", "settingsMenuWin", "launchApprove", "openSysSettings",
        "winLaunchApprove", "openWinStartup", "stillRunningD", "authMacT", "authMacD", "authMacOk", "authMacUn",
        "uacQ", "uacPub", "uacYes", "uacNo", "polkitT", "polkitD", "polkitDu", "polkitBtn", "hiddenIcons",
        // A locked store: names the keychain (macOS) and the keyring (Linux) together, on every platform.
        "errLockedT", "Error_CredentialStoreLocked",
    ];

    public static TheoryData<string> AllLanguages() => new(Languages);

    [Theory]
    [MemberData(nameof(AllLanguages))]
    public void CatalogDefinesEveryRequiredKey(string language)
    {
        var catalog = JsonLocalizer.Catalog(language);
        var missing = RequiredKeys().Where(key => !catalog.ContainsKey(key) || string.IsNullOrWhiteSpace(catalog[key]));
        Assert.Empty(missing);
    }

    [Theory]
    [MemberData(nameof(AllLanguages))]
    public void CatalogKeepsEveryDesignKey(string language)
    {
        var catalog = JsonLocalizer.Catalog(language);
        var missing = DesignKeys().Where(key => !catalog.ContainsKey(key)).ToList();
        Assert.True(missing.Count == 0, "missing design keys: " + string.Join(", ", missing));
    }

    [Fact]
    public void CatalogsHaveTheSameKeysAndPlaceholders()
    {
        var chinese = JsonLocalizer.Catalog(JsonLocalizer.Chinese);
        var english = JsonLocalizer.Catalog(JsonLocalizer.English);
        Assert.Equal(english.Keys.Order(), chinese.Keys.Order());
        foreach (var key in english.Keys)
            Assert.True(Placeholders.Names(english[key]).SetEquals(Placeholders.Names(chinese[key])), $"placeholders differ for {key}");
    }

    [Theory]
    [InlineData("expiredD", "d")]
    [InlineData("teamOffD", "team")]
    [InlineData("switchFailD", "reason")]
    [InlineData("version", "v,b")]
    [InlineData("expiresIn", "t")]
    [InlineData("d_reconnecting", "r0,r")]
    [InlineData("d_on", "r,ms")]
    public void SampleDataBecamePlaceholders(string key, string names)
    {
        foreach (var language in Languages)
            Assert.Equal(names.Split(',').ToHashSet(), Placeholders.Names(JsonLocalizer.Catalog(language)[key]));
    }

    /// <summary>Shared text names no platform, except the platform-specific keys.</summary>
    [Theory]
    [MemberData(nameof(AllLanguages))]
    public void CatalogTextIsPlatformNeutral(string language)
    {
        var platformWord = new Regex(@"Windows|Mac|Linux|Keychain|钥匙串|凭据管理器|访达|Finder|UAC|polkit|资源管理器|File Explorer", RegexOptions.IgnoreCase);
        var offending = JsonLocalizer.Catalog(language)
            .Where(entry => !PlatformKeys.Contains(entry.Key) && platformWord.IsMatch(entry.Value))
            .Select(entry => entry.Key);
        Assert.Empty(offending);
    }

    [Theory]
    [InlineData("zh-CN", JsonLocalizer.Chinese)]
    [InlineData("zh-TW", JsonLocalizer.Chinese)]
    [InlineData("zh-Hans-SG", JsonLocalizer.Chinese)]
    [InlineData("en-GB", JsonLocalizer.English)]
    [InlineData("fr-FR", JsonLocalizer.English)]
    [InlineData("", JsonLocalizer.English)]
    public void LanguageFollowsTheUiCulture(string culture, string expected)
    {
        Assert.Equal(expected, new JsonLocalizer(culture: CultureInfo.GetCultureInfo(culture)).Language);
    }

    [Fact]
    public void MissingKeyReturnsTheKeyAndLogsOnce()
    {
        var log = new TestLog();
        ILocalizer strings = new JsonLocalizer(log, CultureInfo.GetCultureInfo("zh-CN"));
        Assert.Equal("No_Such_Key", strings.Get("No_Such_Key"));
        Assert.Equal("No_Such_Key", strings.Get("No_Such_Key"));
        Assert.Single(log.Lines, l => l.StartsWith("WARN"));
        Assert.Equal("已开启", strings.Get("st_on"));
        Assert.Equal("9:54 后过期", strings.Format("expiresIn", ("t", "9:54")));
        Assert.Equal("Version 1.4.0 (2609)", new JsonLocalizer(culture: CultureInfo.GetCultureInfo("en-US")).Format("version", ("v", "1.4.0"), ("b", "2609")));
        Assert.Equal("网络连接", strings.Format("Error_Unexpected", ("m", "网络连接")).Replace("操作失败：", ""));
    }

    [Fact]
    public void ErrorCodeKeysCoverTheCrate()
    {
        Assert.Contains("Error_ServiceOwnedByAnotherUser", ErrorMessages.AllKeys);
        Assert.Equal(Enum.GetValues<ErrorCode>().Length, ErrorMessages.AllKeys.Count);
        Assert.Contains("Error_SystemProxyFailed", ErrorMessages.AllKeys);
        Assert.Contains("Error_ConnectHealthCheckFailed", ErrorMessages.AllKeys);
    }
}
