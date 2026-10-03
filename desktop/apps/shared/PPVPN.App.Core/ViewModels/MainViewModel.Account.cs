using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>Access to the service (design <c>access</c>): the restricted states replace the overview and node list.</summary>
public enum AccessState { Ok, NoSubscription, Expired, TeamDisabled }

/// <summary>
/// A team in the account menu (radio items) and the Settings team combo. Inactive (disabled or
/// dissolved) teams stay listed with the <c>disabled</c> tag and are not selectable; the
/// personal team shows as <c>personal</c>.
/// </summary>
public sealed record TeamOption(Team Team, string Title, bool IsCurrent, string DisabledTag)
{
    public string Id => Team.Id;
    public bool IsPersonal => Team.Personal;
    public bool IsSelectable => Team.Active;
    public bool HasDisabledTag => DisabledTag.Length > 0;
    public override string ToString() => Title;
}

public sealed partial class MainViewModel
{
    bool _teamsLoaded;

    /// <summary>Where to buy or renew a subscription (<c>Client.purchase_url</c>).</summary>
    public string PurchaseUrl { get; }

    /// <summary>The team's routing-rules page on the web (<c>Client.routing_rules_url</c>).</summary>
    public string RoutingRulesUrl { get; }

    // --- access ------------------------------------------------------------------

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(IsRestricted))] AccessState access;
    /// <summary>No subscription, expired or team disabled: overview ②–⑤ and the node table are replaced by the restricted card.</summary>
    public bool IsRestricted => Access != AccessState.Ok;
    [ObservableProperty] string restrictedTitle = "";
    [ObservableProperty] string restrictedMessage = "";
    /// <summary>NoSubscription / Expired: <c>buy</c> ↗ (<see cref="OpenPurchaseCommand"/>) + <c>refresh</c>.</summary>
    [ObservableProperty] bool canBuy;
    /// <summary>TeamDisabled: <c>switchTeam</c> ▾ opening the account menu.</summary>
    [ObservableProperty] bool canSwitchTeam;
    /// <summary>The restricted card's Refresh is running (the button shows "…").</summary>
    [ObservableProperty] bool isRefreshingAccess;

    /// <summary>The latest profile was rejected but the previous one stays in use: <c>cfgT</c> / <c>cfgD</c> warning.</summary>
    [ObservableProperty] bool configInvalid;
    /// <summary>Signed in and the first profile download for this team has not finished.</summary>
    [ObservableProperty] bool profileLoading;
    /// <summary>Rejected profile and no previous one: <c>invalidT</c> / <c>invalidD</c>.</summary>
    [ObservableProperty] bool invalidNoHistory;

    // --- account ------------------------------------------------------------------

    /// <summary>The account email (or name when the backend has no email).</summary>
    [ObservableProperty] string accountEmail = "";
    /// <summary>Avatar initial for the account button.</summary>
    [ObservableProperty] string avatarInitial = "";
    /// <summary>Account menu header subtitle: <c>serviceExpiresOn</c> {d}, or the restricted title.</summary>
    [ObservableProperty] string accountSubtitle = "";
    /// <summary>The current team's title (<c>personal</c> for the personal team).</summary>
    [ObservableProperty] string teamName = "";
    /// <summary>Service expiry: a long date, "已于 {d} 到期" when expired, "—" without a subscription.</summary>
    [ObservableProperty] string expiresText = "—";
    /// <summary>Show <see cref="ExpiresText"/> in the danger colour.</summary>
    [ObservableProperty] bool isExpired;

    /// <summary>Every team of the account, inactive ones included.</summary>
    public ObservableCollection<TeamOption> Teams { get; } = [];
    /// <summary>Settings team combo (two-way); selecting an inactive team is ignored.</summary>
    [ObservableProperty] TeamOption? selectedTeam;
    [ObservableProperty] bool hasTeamChoice;

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(HasUnreadNotifications))] int unreadNotifications;
    public bool HasUnreadNotifications => UnreadNotifications > 0;
    /// <summary>Bell badge: hidden at 0, "1"–"99", "99+".</summary>
    public string UnreadBadgeText => Formatting.Badge(UnreadNotifications);

    partial void OnUnreadNotificationsChanged(int value) => OnPropertyChanged(nameof(UnreadBadgeText));

    // --- commands --------------------------------------------------------------------

    [RelayCommand]
    void OpenPurchase()
    {
        if (!_services.OpenUrl(PurchaseUrl)) _services.CopyText(PurchaseUrl);
    }

    /// <summary>The restricted card's Refresh (also F5 on Overview / Nodes).</summary>
    [RelayCommand]
    async Task RefreshAccessAsync()
    {
        IsRefreshingAccess = true;
        try
        {
            await RunAsync("refresh_profile", Backend.RefreshProfile);
        }
        finally
        {
            IsRefreshingAccess = false;
        }
    }

    /// <summary>Refresh the node configuration (<c>refreshNodes</c>; F5 on Overview / Nodes).</summary>
    [RelayCommand]
    Task RefreshProfileAsync() => RunAsync("refresh_profile", Backend.RefreshProfile);

    /// <summary>
    /// Switch to <paramref name="team"/> (account menu, Settings combo). Inactive teams and the
    /// current one are ignored. A refused switch keeps the team and shows <c>switchFailT</c>.
    /// </summary>
    [RelayCommand]
    async Task SwitchTeamAsync(TeamOption? team)
    {
        if (team is null || team.Id == Snapshot.Team?.Id) return;
        if (!team.IsSelectable)
        {
            SyncControls();
            return;
        }
        var switched = await RunAsync("switch_team", () => Backend.SwitchTeam(team.Id), "switchFailT",
            message: error => _strings.Format("switchFailD", ("reason", _strings.Message(error))));
        // E.g. TeamDisabled: the team was disabled since the list was loaded.
        if (!switched) await LoadTeamsAsync();
    }

    /// <summary>"Account Settings…" in the account menu.</summary>
    [RelayCommand]
    void OpenAccountSettings() => RequestShow(AppSurface.Settings);

    /// <summary>"Sign Out…": confirm (<see cref="PromptKind.SignOut"/>), then sign out.</summary>
    [RelayCommand]
    async Task SignOutAsync()
    {
        if (!await _prompts.ConfirmAsync(PromptKind.SignOut)) return;
        await RunAsync("logout", Backend.Logout);
    }

    partial void OnSelectedTeamChanged(TeamOption? value)
    {
        if (_applying) return;
        _ = SwitchTeamAsync(value);
    }

    // --- derivation ----------------------------------------------------------------

    void ApplyAccess(ClientSnapshot next)
    {
        var signedIn = next.Auth is AuthState.SignedIn;
        var status = signedIn ? next.ProfileStatus : new ProfileStatus.Loading();
        Access = status switch
        {
            ProfileStatus.NoSubscription => AccessState.NoSubscription,
            ProfileStatus.SubscriptionExpired => AccessState.Expired,
            ProfileStatus.TeamDisabled => AccessState.TeamDisabled,
            _ => AccessState.Ok,
        };
        (RestrictedTitle, RestrictedMessage) = Access switch
        {
            AccessState.NoSubscription => (_strings.Get("noSubT"), _strings.Get("noSubD")),
            AccessState.Expired => (_strings.Get("expiredT"), _strings.Format("expiredD", ("d", ExpiredDate(status) ?? "—"))),
            AccessState.TeamDisabled => (_strings.Get("teamOffT"), _strings.Format("teamOffD", ("team", next.Team?.Name ?? ""))),
            _ => ("", ""),
        };
        CanBuy = Access is AccessState.NoSubscription or AccessState.Expired;
        CanSwitchTeam = Access == AccessState.TeamDisabled;
        ConfigInvalid = status is ProfileStatus.Invalid && next.Profile is not null;
        InvalidNoHistory = status is ProfileStatus.Invalid && next.Profile is null;
        ProfileLoading = signedIn && status is ProfileStatus.Loading;
    }

    void ApplyAccount(ClientSnapshot next)
    {
        AccountEmail = next.Account is { } account ? account.Email ?? account.Name : "";
        AvatarInitial = AccountEmail.Length > 0 ? char.ToUpperInvariant(AccountEmail[0]).ToString() : "";
        TeamName = next.Team is { } team ? TeamTitle(team) : "";

        if (Access == AccessState.Expired)
        {
            var date = ExpiredDate(next.ProfileStatus);
            ExpiresText = date is null ? _strings.Get("expiredT") : _strings.Format("expiredOn", ("d", date));
            IsExpired = true;
        }
        else
        {
            var expires = Formatting.ParseRfc3339(next.Profile?.ExpiresAt);
            ExpiresText = expires is { } at ? Formatting.LongDate(TimeZoneInfo.ConvertTime(at, _time.LocalTimeZone), _strings.Language) : "—";
            IsExpired = false;
        }
        AccountSubtitle = IsRestricted && Access != AccessState.TeamDisabled
            ? RestrictedTitle
            : _strings.Format("serviceExpiresOn", ("d", ExpiresText));

        RebuildTeams(next);
    }

    string? ExpiredDate(ProfileStatus status) =>
        status is ProfileStatus.SubscriptionExpired { ExpiredAt: var at } && Formatting.ParseRfc3339(at) is { } when
            ? Formatting.LongDate(TimeZoneInfo.ConvertTime(when, _time.LocalTimeZone), _strings.Language)
            : null;

    string TeamTitle(Team team) => team.Personal ? _strings.Get("personal") : team.Name;

    TeamOption Option(Team team, ClientSnapshot snapshot) =>
        new(team, TeamTitle(team), team.Id == snapshot.Team?.Id, team.Active ? "" : _strings.Get("disabled"));

    void RebuildTeams(ClientSnapshot next)
    {
        if (Teams.Count == 0) return;
        var options = Teams.Select(t => Option(t.Team, next)).ToList();
        if (!options.SequenceEqual(Teams))
        {
            Teams.Clear();
            foreach (var option in options) Teams.Add(option);
        }
        SelectedTeam = Teams.FirstOrDefault(t => t.Id == next.Team?.Id);
    }

    async Task LoadTeamsAsync()
    {
        try
        {
            var teams = await Backend.Teams();
            _applying = true;
            Teams.Clear();
            foreach (var team in teams) Teams.Add(Option(team, Snapshot));
            HasTeamChoice = Teams.Count > 1;
            SelectedTeam = Teams.FirstOrDefault(t => t.Id == Snapshot.Team?.Id);
        }
        catch (Exception error)
        {
            _log.Warn($"teams failed: {ErrorMessages.Describe(error)}");
        }
        finally
        {
            _applying = false;
        }
        RefreshTray();
    }
}
