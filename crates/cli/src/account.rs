//! Login, the account and the profile download, on `ppvpn-account`.

use std::sync::Arc;

use ppvpn_account::api::{
    Api, ApiConfig, ApiError, SubscriptionProblem, TeamEntry, PATH_PROXY_PROFILE,
};
use ppvpn_account::auth::{Auth, AuthConfig, AuthError, CredentialStore};
use ppvpn_account::session::with_bearer;

use crate::buildinfo::{BuildConfig, Profile};
use crate::env::Env;
use crate::error::{CliError, Exit, Result};

/// The CLI's access-token audience; it sends no product header
/// ([`ApiConfig::cli`]).
pub const TOKEN_AUDIENCE: &str = ppvpn_account::api::CLI_AUDIENCE;

pub fn auth(config: &BuildConfig, env: &Env, store: Arc<dyn CredentialStore>) -> Auth {
    let api = Api::new(ApiConfig::cli(
        &config.api_base,
        locale(env),
        // Only a dev build may talk to a local mock backend.
        config.profile == Profile::Dev,
    ));
    // The page's production host is the API's own; a dev build's page is
    // trusted because it is on the configured API base.
    Auth::new(Arc::new(api), store, AuthConfig::production())
}

/// The POSIX locale as a language tag (`zh_CN.UTF-8` becomes `zh-CN`), or
/// `None` for an unset or C/POSIX locale. The backend localises display
/// names such as node entry and ingress labels by it.
pub fn locale(env: &Env) -> Option<String> {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| env.var(name))?;
    let tag = raw
        .split(['.', '@'])
        .next()
        .unwrap_or_default()
        .replace('_', "-");
    let valid = !tag.is_empty()
        && tag != "C"
        && tag != "POSIX"
        && tag.split('-').all(|part| {
            !part.is_empty() && part.len() <= 8 && part.chars().all(|c| c.is_ascii_alphanumeric())
        })
        && tag.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    valid.then_some(tag)
}

fn relogin(code: &str) -> CliError {
    CliError::new(Exit::Auth, code, "login expired; run ppvpn login again")
}

fn subscription_text(problem: SubscriptionProblem) -> &'static str {
    match problem {
        SubscriptionProblem::TeamDisabled => "the current team is disabled",
        SubscriptionProblem::Expired => "the subscription has expired",
        SubscriptionProblem::NoSubscription => "no active subscription",
        SubscriptionProblem::NodesNotReady => {
            "the subscription's nodes are not ready yet (try again later)"
        }
    }
}

pub fn api_error(err: &ApiError) -> CliError {
    match err {
        ApiError::Transport(_) => CliError::new(
            Exit::Backend,
            "BACKEND_UNAVAILABLE",
            "cannot reach the backend",
        )
        .retryable(),
        ApiError::Status {
            request,
            status,
            code,
            ..
        } => {
            let problem = code.as_deref().filter(|c| !c.is_empty());
            // The subscription problem picks the message only; the exit and
            // the code still follow the status and the backend's code.
            let subscription = err.subscription_problem();
            let message = |fallback: &'static str| subscription.map_or(fallback, subscription_text);
            let retry = |cli: CliError| match subscription {
                Some(SubscriptionProblem::NodesNotReady) => cli.retryable(),
                _ => cli,
            };
            match status.as_u16() {
                401 => relogin(problem.unwrap_or("AUTH_EXPIRED")),
                403 if err.is_team_disabled() => {
                    CliError::new(Exit::Auth, "TEAM_DISABLED", "the current team is disabled")
                }
                403 => retry(CliError::new(
                    Exit::Auth,
                    problem.unwrap_or("FORBIDDEN"),
                    message("the backend denied this operation"),
                )),
                // A profile 404 without a subscription code (an older
                // backend, or a case it does not classify): one message
                // covers both a missing subscription and unpublished nodes.
                404 if request.ends_with(PATH_PROXY_PROFILE) => retry(CliError::new(
                    Exit::Backend,
                    problem.unwrap_or("NOT_FOUND"),
                    message(
                        "no active subscription, or its nodes are not ready yet (try again later)",
                    ),
                )),
                404 => retry(CliError::new(
                    Exit::Backend,
                    problem.unwrap_or("NOT_FOUND"),
                    message("the backend has nothing to serve for this account"),
                )),
                429 | 500..=599 => CliError::new(
                    Exit::Backend,
                    "BACKEND_UNAVAILABLE",
                    "the backend is temporarily unavailable",
                )
                .retryable(),
                _ => retry(CliError::new(
                    Exit::Other,
                    problem.unwrap_or("BACKEND_REJECTED"),
                    message("the backend rejected the request"),
                )),
            }
        }
        ApiError::Decode(what) => CliError::new(
            Exit::Backend,
            "BACKEND_RESPONSE_INVALID",
            format!("unexpected backend response: {what}"),
        ),
        ApiError::Config(what) => CliError::new(
            Exit::Argument,
            "BUILD_CONFIG_INVALID",
            format!("the API base cannot be used: {what}"),
        ),
    }
}

pub fn auth_error(err: &AuthError) -> CliError {
    match err {
        AuthError::Api(api) => api_error(api),
        AuthError::Store(store) if store.locked => CliError::environment(
            "CREDENTIAL_STORE_LOCKED",
            format!("the secret store is locked: {}", store.detail),
        ),
        AuthError::Store(store) => {
            CliError::environment("CREDENTIAL_STORE_UNAVAILABLE", store.detail.clone())
        }
        AuthError::Cancelled => CliError::new(Exit::Other, "CANCELED", "login was cancelled"),
        AuthError::Denied => CliError::new(
            Exit::Auth,
            "AUTH_DEVICE_DENIED",
            "device authorization was denied",
        ),
        AuthError::Expired => CliError::new(
            Exit::Auth,
            "AUTH_DEVICE_EXPIRED",
            "device authorization expired",
        ),
        AuthError::Untrusted(what) => CliError::new(
            Exit::Backend,
            "BACKEND_UNTRUSTED",
            format!("refusing an untrusted backend response: {what}"),
        ),
        AuthError::NoCredential => not_logged_in(),
    }
}

pub fn not_logged_in() -> CliError {
    CliError::new(Exit::Auth, "NOT_LOGGED_IN", "not logged in")
}

/// A fresh access token for the saved login.
pub async fn access_token(auth: &Auth) -> Result<String> {
    let _guard = auth.lock().await;
    match auth.restore().await {
        Ok(Some(signed_in)) => Ok(signed_in.access.token),
        Ok(None) => Err(not_logged_in()),
        Err(err) => Err(auth_error(&err)),
    }
}

/// Downloads the proxy profile as raw bytes; core parses and validates it.
pub async fn download_profile(auth: &Auth) -> Result<Vec<u8>> {
    let token = access_token(auth).await?;
    // The token may have been revoked or rotated elsewhere: refresh once,
    // then give up with the login error.
    let result = with_bearer(
        token.clone(),
        |token| async move { auth.api().proxy_profile(&token).await },
        |_| async move {
            let _guard = auth.lock().await;
            auth.refresh_or_clear()
                .await
                .map(|access| access.token)
                .map_err(|e| Err(auth_error(&e)))
        },
        Ok,
    )
    .await;
    match result {
        Ok(profile) => Ok(profile),
        Err(Ok(err)) => Err(profile_error(auth, &token, &err).await),
        Err(Err(err)) => Err(err),
    }
}

/// A refused profile download, with the backend's own words and the team
/// the login is on: the backend serves the profile of the account's current
/// team, which can be changed elsewhere (the dashboard, the desktop app).
async fn profile_error(auth: &Auth, token: &str, err: &ApiError) -> CliError {
    let mut cli = api_error(err);
    let ApiError::Status { status, detail, .. } = err else {
        return cli;
    };
    if status.as_u16() == 401 || status.as_u16() == 429 || status.is_server_error() {
        return cli;
    }
    let mut notes = Vec::new();
    if let Some(detail) = detail.as_deref() {
        if !cli.message.to_lowercase().contains(&detail.to_lowercase()) {
            notes.push(format!("backend: {detail}"));
        }
    }
    // Best effort: without the list the error stands as it is.
    let team = auth.api().teams(token).await.ok().and_then(|teams| {
        teams
            .into_iter()
            .find(|team| team.is_default)
            .map(|team| team_label(&team))
    });
    if let Some(team) = &team {
        notes.push(format!("team: {team}"));
    }
    if !notes.is_empty() {
        cli.message = format!("{} ({})", cli.message, notes.join("; "));
    }
    if team.is_some() {
        cli.message.push_str("; ppvpn team lists your teams");
    }
    cli
}

/// `name (id)`, or the ID alone for a team without a name.
pub fn team_label(team: &TeamEntry) -> String {
    if team.name.is_empty() {
        team.id.clone()
    } else {
        format!("{} ({})", team.name, team.id)
    }
}

/// A team as `--json` shows it.
pub fn team_json(team: &TeamEntry) -> serde_json::Value {
    serde_json::json!({
        "id": team.id,
        "name": team.name,
        "personal": team.is_personal,
        "current": team.is_default,
        "status": team.status.as_deref().unwrap_or("active"),
    })
}

/// The team `wanted` names: an exact ID, else an exact name that only one
/// team has.
pub fn find_team<'a>(teams: &'a [TeamEntry], wanted: &str) -> Result<&'a TeamEntry> {
    if let Some(team) = teams.iter().find(|team| team.id == wanted) {
        return Ok(team);
    }
    let named: Vec<_> = teams.iter().filter(|team| team.name == wanted).collect();
    match named[..] {
        [team] => Ok(team),
        [] => Err(CliError::new(
            Exit::Argument,
            "TEAM_NOT_FOUND",
            format!("no team has the ID or name {wanted:?}; ppvpn team lists them"),
        )),
        _ => Err(CliError::new(
            Exit::Argument,
            "TEAM_AMBIGUOUS",
            format!("several teams are named {wanted:?}; use the team ID"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::Os;

    #[test]
    fn the_verification_page_is_on_the_production_api_host() {
        let host = crate::buildinfo::PRODUCTION_API_BASE.trim_start_matches("https://");
        assert_eq!(AuthConfig::production().verification_host, host);
        assert_eq!(TOKEN_AUDIENCE, "cli");
    }

    #[test]
    fn locale_tags() {
        let tag = |vars: &[(&str, &str)]| locale(&Env::with_vars(Os::Linux, vars));
        assert_eq!(tag(&[("LANG", "zh_CN.UTF-8")]).as_deref(), Some("zh-CN"));
        assert_eq!(
            tag(&[("LANG", "fr_FR.UTF-8"), ("LC_ALL", "en_US")]).as_deref(),
            Some("en-US")
        );
        assert_eq!(
            tag(&[("LC_MESSAGES", "de_DE@euro")]).as_deref(),
            Some("de-DE")
        );
        assert_eq!(
            tag(&[("LANG", "zh_Hant_TW.UTF-8")]).as_deref(),
            Some("zh-Hant-TW")
        );
        for unset in ["C", "POSIX", "C.UTF-8", "bad value\r\n", "-x"] {
            assert_eq!(tag(&[("LANG", unset)]), None, "{unset:?}");
        }
        assert_eq!(tag(&[]), None);
    }

    #[test]
    fn auth_errors_map_to_login_and_environment_exits() {
        assert_eq!(auth_error(&AuthError::NoCredential).code, "NOT_LOGGED_IN");
        assert_eq!(auth_error(&AuthError::NoCredential).exit_code(), 3);
        assert_eq!(auth_error(&AuthError::Denied).exit_code(), 3);
        assert_eq!(auth_error(&AuthError::Expired).code, "AUTH_DEVICE_EXPIRED");
        assert_eq!(auth_error(&AuthError::Untrusted("x")).exit_code(), 4);
        assert_eq!(api_error(&ApiError::Decode("x")).exit_code(), 4);
        assert_eq!(api_error(&ApiError::Config("x")).exit_code(), 2);
    }
}
