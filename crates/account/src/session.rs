//! A signed-in client between backend calls: when to renew its access
//! token, how to retry a call the backend refused with 401, and how often to
//! re-read a credential store that could not be read.
//!
//! The host keeps the session itself (the desktop per app session, the CLI
//! per command); these are the rules both follow.

use std::future::Future;
use std::time::Duration;

use crate::api::ApiError;

/// Renew an access token this long before it expires.
pub const ACCESS_TOKEN_MARGIN: Duration = Duration::from_secs(60);

/// Runs a bearer call with `token`. When the backend answers 401 (the token
/// was revoked or rotated elsewhere), `renew` gets the refused token and
/// returns a new one, and the call runs once more. Other results, and the
/// second call's, go through `map`.
pub async fn with_bearer<T, E, F, Fut, R, RFut>(
    token: String,
    call: F,
    renew: R,
    map: impl FnOnce(ApiError) -> E,
) -> Result<T, E>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<T, ApiError>>,
    R: FnOnce(String) -> RFut,
    RFut: Future<Output = Result<String, E>>,
{
    match call(token.clone()).await {
        Err(error) if error.is_unauthorized() => {
            let token = renew(token).await?;
            call(token).await.map_err(map)
        }
        other => other.map_err(map),
    }
}

/// How often to retry reading a credential store that failed (e.g. a login
/// item started before the keychain was unlocked): every `fast` for the
/// first `fast_window`, then every `slow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetrySchedule {
    pub fast: Duration,
    pub slow: Duration,
    pub fast_window: Duration,
}

impl RetrySchedule {
    /// The credential-store retry of a launch-time restore.
    pub const CREDENTIAL_STORE: Self = Self {
        fast: Duration::from_secs(30),
        slow: Duration::from_secs(5 * 60),
        fast_window: Duration::from_secs(10 * 60),
    };

    /// Delay before the next attempt, `elapsed` after the first failure.
    pub fn delay(&self, elapsed: Duration) -> Duration {
        if elapsed < self.fast_window {
            self.fast
        } else {
            self.slow
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use reqwest::StatusCode;

    use super::*;

    fn status(code: u16) -> ApiError {
        ApiError::Status {
            request: "GET /api/v1/users/me".into(),
            status: StatusCode::from_u16(code).unwrap(),
            code: None,
            expired_at: None,
            detail: None,
        }
    }

    #[test]
    fn credential_retry_schedule_is_fast_then_slow() {
        let schedule = RetrySchedule::CREDENTIAL_STORE;
        let (fast, slow) = (Duration::from_secs(30), Duration::from_secs(300));
        assert_eq!(schedule.delay(Duration::ZERO), fast);
        assert_eq!(schedule.delay(Duration::from_secs(599)), fast);
        assert_eq!(schedule.delay(Duration::from_secs(600)), slow);
    }

    /// Runs `with_bearer` against scripted answers; returns the result and
    /// the tokens each call and renewal saw.
    async fn run(
        answers: Vec<Result<u32, ApiError>>,
        renewed: Result<&'static str, &'static str>,
    ) -> (Result<u32, String>, Vec<String>, Vec<String>) {
        let answers = Mutex::new(answers.into_iter());
        let calls = Mutex::new(Vec::new());
        let renewals = Mutex::new(Vec::new());
        let result = with_bearer(
            "t1".to_string(),
            |token| {
                calls.lock().unwrap().push(token);
                let answer = answers.lock().unwrap().next().unwrap();
                async move { answer }
            },
            |refused| {
                renewals.lock().unwrap().push(refused);
                async move { renewed.map(str::to_string).map_err(str::to_string) }
            },
            |error| format!("{error:?}"),
        )
        .await;
        (
            result,
            calls.into_inner().unwrap(),
            renewals.into_inner().unwrap(),
        )
    }

    #[tokio::test]
    async fn a_401_renews_the_token_and_retries_once() {
        let (result, calls, renewals) = run(vec![Err(status(401)), Ok(7)], Ok("t2")).await;
        assert_eq!(result, Ok(7));
        assert_eq!(calls, ["t1", "t2"]);
        assert_eq!(renewals, ["t1"]);

        // A second 401 is the caller's error, not another retry.
        let (result, calls, _) = run(vec![Err(status(401)), Err(status(401))], Ok("t2")).await;
        assert!(result.unwrap_err().contains("401"));
        assert_eq!(calls.len(), 2);
    }

    #[tokio::test]
    async fn other_answers_and_failed_renewals_are_not_retried() {
        let (result, calls, renewals) = run(vec![Ok(1)], Ok("t2")).await;
        assert_eq!((result, calls.len(), renewals.len()), (Ok(1), 1, 0));

        let (result, calls, renewals) = run(vec![Err(status(403))], Ok("t2")).await;
        assert!(result.unwrap_err().contains("403"));
        assert_eq!((calls.len(), renewals.len()), (1, 0));

        let (result, calls, _) = run(vec![Err(status(401))], Err("signed out")).await;
        assert_eq!(result, Err("signed out".to_string()));
        assert_eq!(calls.len(), 1);
    }
}
