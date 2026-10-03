//! Log files, rotated daily (UTC): `<log_dir>/ppvpn-client.YYYY-MM-DD.log`
//! from the app, `ppvpn-push-agent.YYYY-MM-DD.log` from the push agent, next
//! to the core's `ppvpn-core.YYYY-MM-DD.log`. Files of any of these kinds
//! older than [`KEEP_DAYS`] are deleted at startup.
//!
//! Level `info` by default; `PPVPN_LOG` accepts a `tracing` filter such as
//! `debug` or `ppvpn_client=trace`. With the Rust core (`rust-core`), sail's
//! and the engine's events go to the engine's own log through their tracing
//! layers, never into this file; the filter applies to this file only, so it
//! cannot drop the engine's debug lines. Never log tokens, credentials, local-proxy
//! passwords or profile bodies: the log pages of the apps show these files.

use std::path::Path;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_appender::rolling::{Builder, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

/// Log files kept per kind, today included.
pub(crate) const KEEP_DAYS: u64 = 7;
/// Environment variable overriding the default filter.
pub(crate) const LOG_ENV: &str = "PPVPN_LOG";
const PREFIXES: &[&str] = &["ppvpn-client", "ppvpn-core", "ppvpn-push-agent"];
/// File prefix of the app's own log.
pub(crate) const CLIENT_PREFIX: &str = "ppvpn-client";
/// File prefix of the push agent's log.
pub(crate) const PUSH_AGENT_PREFIX: &str = "ppvpn-push-agent";

static INSTALLED: OnceLock<()> = OnceLock::new();

/// The filter directive for an optional `PPVPN_LOG` value.
pub(crate) fn filter_directive(env: Option<&str>) -> String {
    match env.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value) if tracing_subscriber::EnvFilter::try_new(value).is_ok() => value.to_string(),
        _ => "info".to_string(),
    }
}

/// The app log's filter: `directive` without the engine's targets, which
/// have a log of their own. The more specific `=off` directives win over
/// whatever `directive` says.
fn file_directive(directive: &str) -> String {
    format!("{directive},sail=off,ppvpn_core=off")
}

/// Days since the Unix epoch of a `YYYY-MM-DD` date.
pub(crate) fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    // Howard Hinnant's days_from_civil.
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Epoch day encoded in `<prefix>.YYYY-MM-DD.log`, for our prefixes only.
fn log_file_day(name: &str) -> Option<i64> {
    let date = PREFIXES.iter().find_map(|prefix| {
        name.strip_prefix(prefix)?
            .strip_prefix('.')?
            .strip_suffix(".log")
    })?;
    let mut parts = date.split('-');
    let (year, month, day) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() || year.len() != 4 || month.len() != 2 || day.len() != 2 {
        return None;
    }
    let (year, month, day) = (year.parse().ok()?, month.parse().ok()?, day.parse().ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

/// Names among `names` that are our daily logs older than [`KEEP_DAYS`].
pub(crate) fn expired_logs<'a>(
    names: impl IntoIterator<Item = &'a str>,
    today: i64,
) -> Vec<String> {
    names
        .into_iter()
        .filter(|name| log_file_day(name).is_some_and(|day| today - day >= KEEP_DAYS as i64))
        .map(str::to_string)
        .collect()
}

fn prune(log_dir: &Path) {
    let today = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86_400) as i64;
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };
    let names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    for name in expired_logs(names.iter().map(String::as_str), today) {
        let _ = std::fs::remove_file(log_dir.join(name));
    }
}

/// Installs the process-wide file subscriber once; later calls (a second
/// `Client`, or an app that installed its own subscriber) are no-ops.
pub(crate) fn install(log_dir: &str) {
    install_with_prefix(log_dir, CLIENT_PREFIX);
}

/// [`install`] writing `<log_dir>/<prefix>.YYYY-MM-DD.log`.
pub(crate) fn install_with_prefix(log_dir: &str, prefix: &'static str) {
    INSTALLED.get_or_init(|| {
        let dir = Path::new(log_dir);
        if std::fs::create_dir_all(dir).is_err() {
            return;
        }
        prune(dir);
        let Ok(appender) = Builder::new()
            .rotation(Rotation::DAILY)
            .filename_prefix(prefix)
            .filename_suffix("log")
            .max_log_files(KEEP_DAYS as usize)
            .build(dir)
        else {
            return;
        };
        let directive = filter_directive(std::env::var(LOG_ENV).ok().as_deref());
        let file = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(appender)
            .with_filter(tracing_subscriber::EnvFilter::new(file_directive(
                &directive,
            )));
        let registry = tracing_subscriber::registry().with(file);
        // Global (not thread-local): sail's and the engine's worker threads
        // log through it.
        #[cfg(feature = "rust-core")]
        let registry = registry
            .with(sail::embed::tracing_layer())
            .with(ppvpn_core::tracing_layer());
        let _ = registry.try_init();
        // A panic inside a background task is otherwise only printed to
        // stderr, which GUI apps discard; keep it in the log file too.
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            tracing::error!("panic: {info}");
            previous(info);
        }));
        tracing::info!(version = env!("CARGO_PKG_VERSION"), "{prefix} started");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_log_leaves_the_engine_targets_to_the_engine() {
        for directive in ["info", "debug", "ppvpn_client=trace,sail=debug"] {
            let file = file_directive(directive);
            assert!(file.ends_with(",sail=off,ppvpn_core=off"), "{file}");
            assert!(
                tracing_subscriber::EnvFilter::try_new(&file).is_ok(),
                "{file}"
            );
        }
    }

    #[test]
    fn filter_defaults_to_info_and_accepts_valid_overrides() {
        assert_eq!(filter_directive(None), "info");
        assert_eq!(filter_directive(Some("")), "info");
        assert_eq!(filter_directive(Some(" debug ")), "debug");
        assert_eq!(
            filter_directive(Some("ppvpn_client=trace")),
            "ppvpn_client=trace"
        );
        assert_eq!(filter_directive(Some("=[bad")), "info");
    }

    #[test]
    fn only_our_logs_older_than_a_week_expire() {
        let today = days_from_civil(2026, 9, 29);
        assert_eq!(today, 1_790_640_000 / 86_400);
        let names = [
            "ppvpn-client.2026-09-29.log",
            "ppvpn-client.2026-09-23.log",
            "ppvpn-client.2026-09-22.log",
            "ppvpn-core.2026-08-01.log",
            "ppvpn-core.2026-09-28.log",
            "other.2020-01-01.log",
            "ppvpn-client.log",
            "ppvpn-client.2026-13-01.log",
        ];
        assert_eq!(
            expired_logs(names, today),
            vec!["ppvpn-client.2026-09-22.log", "ppvpn-core.2026-08-01.log"]
        );
    }

    #[test]
    fn install_is_idempotent_and_writes_the_daily_file() {
        let dir = std::env::temp_dir().join(format!("ppvpn-log-test-{}", uuid::Uuid::new_v4()));
        let dir_text = dir.to_string_lossy().into_owned();
        install(&dir_text);
        install(&dir_text);
        // Only the first test process-wide install wins; when it was ours the
        // file exists with the expected name.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().into_string().unwrap();
                assert!(log_file_day(&name).is_some(), "unexpected file {name}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
