using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

public enum AuthStage { Restoring, SignedOut, Awaiting, SignedIn }

/// <summary>
/// Login error kinds (design: expired / denied / network), plus any other failure, and
/// <see cref="StoreLocked"/>: the keychain / keyring holding the saved login is locked
/// (<see cref="ErrorCode.CredentialStoreLocked"/>); the login comes back once it is unlocked.
/// </summary>
public enum LoginErrorKind { None, Expired, Denied, Network, Other, StoreLocked }

public sealed partial class MainViewModel
{
    DateTimeOffset? _loginExpiresAt;
    string? _countdownCode;
    LoginErrorKind _localLoginError;
    string? _localLoginMessage;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsRestoring), nameof(ShowLogin), nameof(IsSignedIn), nameof(IsAwaiting), nameof(IsLoginIdle))]
    AuthStage stage;

    public bool IsRestoring => Stage == AuthStage.Restoring;
    /// <summary>The login page (signed out, waiting or error). The nav row, bell and account are hidden.</summary>
    public bool ShowLogin => Stage is AuthStage.SignedOut or AuthStage.Awaiting;
    public bool IsSignedIn => Stage == AuthStage.SignedIn;
    public bool IsAwaiting => Stage == AuthStage.Awaiting;
    public bool IsLoginIdle => Stage == AuthStage.SignedOut;

    [ObservableProperty] string userCode = "";
    [ObservableProperty] string verificationUrl = "";
    /// <summary>The browser could not be opened: show the URL (a platform extra, not in the design).</summary>
    [ObservableProperty] bool browserNotOpened;
    /// <summary><c>expiresIn</c>: "9:54 后过期".</summary>
    [ObservableProperty] string countdownText = "";
    [ObservableProperty] bool isStartingLogin;

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(HasLoginError))] LoginErrorKind loginErrorKind;
    [ObservableProperty] string loginErrorTitle = "";
    [ObservableProperty] string loginErrorMessage = "";
    /// <summary>
    /// <c>tryAgain</c> after a network error or a locked credential store, otherwise
    /// <c>signInAgain</c>; runs <see cref="SignInCommand"/>.
    /// </summary>
    [ObservableProperty] string loginRetryText = "";

    public bool HasLoginError => LoginErrorKind != LoginErrorKind.None;

    /// <summary>
    /// The login page's primary button. While the credential store is locked
    /// (<see cref="LoginErrorKind.StoreLocked"/>) it reads the store again instead of starting a
    /// new sign-in, which would hit the same lock when saving; it only signs in when no restore
    /// is pending.
    /// </summary>
    [RelayCommand]
    async Task SignInAsync()
    {
        if (LoginErrorKind == LoginErrorKind.StoreLocked && RetryCredentialRestore()) return;
        IsStartingLogin = true;
        SetLocalLoginError(LoginErrorKind.None, null);
        ApplyLoginError(Snapshot);
        try
        {
            var code = await Backend.AuthStart();
            _log.Info($"device login: code issued, browser_opened={code.BrowserOpened}");
        }
        catch (ClientException.Cancelled)
        {
            // Superseded by a newer sign-in or cancelled; the snapshot says what happened.
            _log.Info("auth_start: cancelled");
        }
        catch (Exception error)
        {
            _log.Error($"auth_start failed: {ErrorMessages.Describe(error)}");
            SetLocalLoginError(LoginKind(ErrorMessages.Code(error)), _strings.Message(error));
            ApplyLoginError(Snapshot);
        }
        finally
        {
            IsStartingLogin = false;
        }
    }

    /// <summary>Ask the crate to read the locked credential store now; false when nothing is pending.</summary>
    bool RetryCredentialRestore()
    {
        var pending = Backend.RetryCredentialRestore();
        _log.Info($"credential store: retry requested, pending={pending}");
        return pending;
    }

    [RelayCommand]
    void CancelSignIn() => Backend.AuthCancel();

    [RelayCommand]
    void ReopenBrowser()
    {
        if (!_services.OpenUrl(VerificationUrl)) BrowserNotOpened = true;
    }

    [RelayCommand]
    void CopyCode() => _services.CopyText(UserCode);

    [RelayCommand]
    void CopyVerificationUrl() => _services.CopyText(VerificationUrl);

    void ApplyAuth(ClientSnapshot previous, ClientSnapshot next)
    {
        Stage = next.Auth switch
        {
            AuthState.Restoring => AuthStage.Restoring,
            AuthState.SignedOut => AuthStage.SignedOut,
            AuthState.AwaitingBrowser => AuthStage.Awaiting,
            _ => AuthStage.SignedIn,
        };

        if (next.Auth is AuthState.AwaitingBrowser { Code: var code })
        {
            UserCode = code.UserCode;
            VerificationUrl = code.VerificationUrl;
            BrowserNotOpened = !code.BrowserOpened;
            if (_countdownCode != code.UserCode)
            {
                _countdownCode = code.UserCode;
                _loginExpiresAt = _time.GetUtcNow().AddSeconds(code.ExpiresInSecs);
            }
            UpdateCountdown();
        }
        else
        {
            _countdownCode = null;
            _loginExpiresAt = null;
        }

        if (next.Auth is AuthState.SignedIn) SetLocalLoginError(LoginErrorKind.None, null);
        ApplyLoginError(next);

        if (next.Auth is AuthState.SignedOut && previous.Auth is AuthState.SignedIn)
        {
            _teamsLoaded = false;
            Teams.Clear();
            HasTeamChoice = false;
            UpRate = DownRate = Formatting.Rate(0);
        }
    }

    void ApplyLoginError(ClientSnapshot snapshot)
    {
        var kind = LoginErrorKind.None;
        string? message = null;
        if (snapshot.Auth is AuthState.SignedOut)
        {
            if (_localLoginError != LoginErrorKind.None)
            {
                kind = _localLoginError;
                message = _localLoginMessage;
            }
            else if (snapshot.LastError is { } error)
            {
                kind = LoginKind(error.Code);
                message = _strings.Message(error);
            }
        }
        LoginErrorKind = kind;
        (LoginErrorTitle, LoginErrorMessage) = kind switch
        {
            LoginErrorKind.Expired => (_strings.Get("errExpiredT"), _strings.Get("errExpiredD")),
            LoginErrorKind.Denied => (_strings.Get("errDeniedT"), _strings.Get("errDeniedD")),
            LoginErrorKind.Network => (_strings.Get("errNetT"), _strings.Get("errNetD")),
            LoginErrorKind.StoreLocked => (_strings.Get("errLockedT"), _strings.Message(ErrorCode.CredentialStoreLocked)),
            LoginErrorKind.Other => (_strings.Get("errorT"), message ?? ""),
            _ => ("", ""),
        };
        LoginRetryText = kind is LoginErrorKind.Network or LoginErrorKind.StoreLocked ? _strings.Get("tryAgain") : _strings.Get("signInAgain");
    }

    static LoginErrorKind LoginKind(ErrorCode? code) => code switch
    {
        ErrorCode.AuthExpired => LoginErrorKind.Expired,
        ErrorCode.AuthDenied => LoginErrorKind.Denied,
        ErrorCode.NetworkUnreachable or ErrorCode.ServerUnavailable or ErrorCode.RateLimited => LoginErrorKind.Network,
        ErrorCode.CredentialStoreLocked => LoginErrorKind.StoreLocked,
        _ => LoginErrorKind.Other,
    };

    void SetLocalLoginError(LoginErrorKind kind, string? message)
    {
        _localLoginError = kind;
        _localLoginMessage = message;
    }

    void TickCountdown()
    {
        if (Stage != AuthStage.Awaiting || _loginExpiresAt is null) return;
        UpdateCountdown();
        if (_time.GetUtcNow() < _loginExpiresAt) return;
        // The code expired locally: show the expired error even before the crate reports it.
        _log.Info("device login: code expired (local countdown)");
        SetLocalLoginError(LoginErrorKind.Expired, null);
        _loginExpiresAt = null;
        Backend.AuthCancel();
    }

    void UpdateCountdown()
    {
        if (_loginExpiresAt is not { } expiresAt) return;
        CountdownText = _strings.Format("expiresIn", ("t", Formatting.Countdown(expiresAt - _time.GetUtcNow())));
    }
}
