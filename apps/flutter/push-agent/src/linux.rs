//! Linux, as apps/linux/PPVPN.PushAgent (Program.cs, DesktopNotifier.cs):
//! - One per user: an flock on `$XDG_RUNTIME_DIR/ppvpn-push-agent.lock`
//!   (else in the data directory). It goes with the process, so a crashed
//!   agent never blocks the next one.
//! - SIGTERM, SIGINT and SIGHUP stop it, and so does the loss of the
//!   session bus (nothing left to show notifications on).
//! - Notifications go to org.freedesktop.Notifications with the
//!   desktop-entry hint, so the shell shows them under the app's name and
//!   icon. A click (the default action) runs
//!   `ppvpn --open-notification <push id>` from the agent's directory, which
//!   the running app receives through its command line and resolves (link,
//!   inbox message or the shown push) itself.
//! - Pushes that found no notification server are offered again as soon as
//!   one appears.

use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use ppvpn_client::{
    MessageSeverity, PushAgent, PushAgentConfig, PushAgentListener, PushAgentState, PushMessage,
};
use tokio::runtime::Runtime;
use tokio::signal::unix::{signal, SignalKind};
use zbus::export::futures_util::StreamExt;
use zbus::message::Type as MessageType;
use zbus::zvariant::Value;
use zbus::{Connection, MatchRule, MessageStream};

const LOCK_FILE: &str = "ppvpn-push-agent.lock";
const SERVER_NAME: &str = "org.freedesktop.Notifications";
const SERVER_PATH: &str = "/org/freedesktop/Notifications";
const DESKTOP_ENTRY: &str = "com.peakpassvpn.ppvpn.desktop";
/// The app, next to the agent (/usr/lib/ppvpn/ppvpn).
const APP: &str = "ppvpn";
const CALL_TIMEOUT: Duration = Duration::from_secs(10);
/// Notifications still on screen (or in the shell's list) that a click can
/// open.
const MAX_TRACKED: usize = 200;

pub(crate) fn main() -> ExitCode {
    let (data_dir, log_dir) = match crate::dirs::dirs() {
        Ok(dirs) => dirs,
        Err(error) => {
            eprintln!("ppvpn-push-agent: {error}");
            return ExitCode::FAILURE;
        }
    };
    let lock_dir = runtime_dir().unwrap_or_else(|| PathBuf::from(&data_dir));
    let _lock = match lock(&lock_dir.join(LOCK_FILE)) {
        Ok(Some(file)) => file,
        Ok(None) => return ExitCode::SUCCESS, // already running
        Err(error) => {
            eprintln!("ppvpn-push-agent: lock: {error}");
            return ExitCode::FAILURE;
        }
    };

    // D-Bus runs on a runtime of its own: `PushAgent::run` blocks this
    // thread, and the client calls the listener from its own threads.
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("ppvpn-push-dbus")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("ppvpn-push-agent: {error}");
            return ExitCode::FAILURE;
        }
    };
    let app = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(APP)))
        .unwrap_or_else(|| PathBuf::from(APP));
    let (notifier, signals) = match runtime.block_on(Notifier::connect(runtime.handle(), app)) {
        Ok(connected) => connected,
        Err(error) => {
            eprintln!("ppvpn-push-agent: no D-Bus session bus: {error}");
            return ExitCode::FAILURE;
        }
    };
    let notifier = Arc::new(notifier);

    // Installs the log (ppvpn-push-agent.* in the log directory).
    let agent = PushAgent::new(
        PushAgentConfig {
            data_dir,
            log_dir,
            platform: "linux".into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
        },
        Arc::new(Listener(notifier.clone())),
    );
    notifier.watch(signals, &agent);
    stop_on_signals(&runtime, &agent);

    agent.run();
    runtime.shutdown_timeout(Duration::from_secs(1));
    ExitCode::SUCCESS
}

fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
}

/// The instance lock; `None` when another agent holds it.
fn lock(path: &Path) -> std::io::Result<Option<File>> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Stops the agent on SIGTERM, SIGINT and SIGHUP instead of dying, so that
/// `run` returns and the process exits on its own.
fn stop_on_signals(runtime: &Runtime, agent: &Arc<PushAgent>) {
    let _context = runtime.enter();
    for kind in [
        SignalKind::terminate(),
        SignalKind::interrupt(),
        SignalKind::hangup(),
    ] {
        let mut stream = match signal(kind) {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!("push agent: signal handler: {error}");
                continue;
            }
        };
        let agent = agent.clone();
        runtime.spawn(async move {
            while stream.recv().await.is_some() {
                agent.stop();
            }
        });
    }
}

/// Called by the client on its own threads.
struct Listener(Arc<Notifier>);

impl PushAgentListener for Listener {
    fn on_push(&self, message: PushMessage) -> bool {
        // Blocks this thread (the client's) until the server answered.
        let (sender, receiver) = mpsc::channel();
        let notifier = self.0.clone();
        self.0.runtime.spawn(async move {
            let shown = tokio::time::timeout(CALL_TIMEOUT, notifier.show(&message)).await;
            let _ = sender.send((message.id, shown));
        });
        match receiver.recv() {
            Ok((_, Ok(Ok(())))) => true,
            Ok((id, Ok(Err(error)))) => {
                // No notification server (ServiceUnknown), or the bus is gone.
                tracing::info!("push {id}: notify failed: {error}");
                false
            }
            Ok((id, Err(_))) => {
                tracing::info!("push {id}: notify timed out");
                false
            }
            Err(_) => false,
        }
    }

    // The state is in the agent's log; nothing to show for it.
    fn on_state(&self, _state: PushAgentState) {}
}

/// The signal subscriptions, made before the agent starts so that no click
/// is missed; [`Notifier::watch`] follows them.
struct Signals {
    owner: MessageStream,
    token: MessageStream,
    action: MessageStream,
    closed: MessageStream,
}

/// org.freedesktop.Notifications client.
struct Notifier {
    connection: Connection,
    runtime: tokio::runtime::Handle,
    app: PathBuf,
    tracked: Mutex<Tracked>,
}

#[derive(Default)]
struct Tracked {
    /// The server's body-markup capability, once asked.
    body_markup: Option<bool>,
    /// Notification id → push id.
    shown: HashMap<u32, u64>,
    /// Sent just before ActionInvoked by servers that support XDG
    /// activation (Wayland focus).
    activation_tokens: HashMap<u32, String>,
}

impl Tracked {
    /// Servers that never report closing (some keep a history) must not
    /// grow the map.
    fn trim(&mut self) {
        if self.shown.len() <= MAX_TRACKED {
            return;
        }
        let mut ids: Vec<u32> = self.shown.keys().copied().collect();
        ids.sort_unstable();
        for id in &ids[..ids.len() - MAX_TRACKED] {
            self.shown.remove(id);
            self.activation_tokens.remove(id);
        }
    }
}

async fn signal_stream(
    connection: &Connection,
    path: &'static str,
    interface: &'static str,
    member: &'static str,
    arg0: Option<&'static str>,
) -> zbus::Result<MessageStream> {
    let mut rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .path(path)?
        .interface(interface)?
        .member(member)?;
    if let Some(arg0) = arg0 {
        rule = rule.arg(0, arg0)?;
    }
    MessageStream::for_match_rule(rule.build(), connection, None).await
}

impl Notifier {
    async fn connect(
        runtime: &tokio::runtime::Handle,
        app: PathBuf,
    ) -> zbus::Result<(Self, Signals)> {
        let connection = Connection::session().await?;
        let signals = Signals {
            owner: signal_stream(
                &connection,
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameOwnerChanged",
                Some(SERVER_NAME),
            )
            .await?,
            token: signal_stream(
                &connection,
                SERVER_PATH,
                SERVER_NAME,
                "ActivationToken",
                None,
            )
            .await?,
            action: signal_stream(&connection, SERVER_PATH, SERVER_NAME, "ActionInvoked", None)
                .await?,
            closed: signal_stream(
                &connection,
                SERVER_PATH,
                SERVER_NAME,
                "NotificationClosed",
                None,
            )
            .await?,
        };
        let notifier = Self {
            connection,
            runtime: runtime.clone(),
            app,
            tracked: Mutex::new(Tracked::default()),
        };
        Ok((notifier, signals))
    }

    fn tracked(&self) -> MutexGuard<'_, Tracked> {
        self.tracked.lock().unwrap()
    }

    /// Follows the server's signals until the session bus goes away.
    fn watch(self: &Arc<Self>, signals: Signals, agent: &Arc<PushAgent>) {
        let Signals {
            mut owner,
            mut token,
            mut action,
            mut closed,
        } = signals;

        let (notifier, agent) = (self.clone(), agent.clone());
        self.runtime.spawn(async move {
            while let Some(message) = owner.next().await {
                let Ok(message) = message else { continue };
                let Ok((name, _, new_owner)) =
                    message.body().deserialize::<(String, String, String)>()
                else {
                    continue;
                };
                if name != SERVER_NAME || new_owner.is_empty() {
                    continue;
                }
                tracing::info!("push agent: notification server appeared");
                // Another server, other capabilities.
                notifier.tracked().body_markup = None;
                agent.retry_pending();
            }
            // Every stream of the connection ends with it.
            tracing::info!("push agent: session bus lost");
            agent.stop();
        });

        let notifier = self.clone();
        self.runtime.spawn(async move {
            while let Some(message) = token.next().await {
                let Ok(message) = message else { continue };
                let Ok((id, value)) = message.body().deserialize::<(u32, String)>() else {
                    continue;
                };
                let mut tracked = notifier.tracked();
                if tracked.shown.contains_key(&id) {
                    tracked.activation_tokens.insert(id, value);
                }
            }
        });

        let notifier = self.clone();
        self.runtime.spawn(async move {
            while let Some(message) = action.next().await {
                let Ok(message) = message else { continue };
                let Ok((id, name)) = message.body().deserialize::<(u32, String)>() else {
                    continue;
                };
                if name == "default" {
                    notifier.open(id);
                }
            }
        });

        let notifier = self.clone();
        self.runtime.spawn(async move {
            while let Some(message) = closed.next().await {
                let Ok(message) = message else { continue };
                let Ok((id, _reason)) = message.body().deserialize::<(u32, u32)>() else {
                    continue;
                };
                let mut tracked = notifier.tracked();
                tracked.shown.remove(&id);
                tracked.activation_tokens.remove(&id);
            }
        });
    }

    /// Hands `message` to the notification server.
    async fn show(&self, message: &PushMessage) -> zbus::Result<()> {
        let cached = self.tracked().body_markup;
        let markup = match cached {
            Some(markup) => markup,
            None => {
                let markup = self.has_body_markup().await?;
                self.tracked().body_markup = Some(markup);
                markup
            }
        };
        let body = if markup {
            escape_markup(&message.body)
        } else {
            message.body.clone()
        };
        let urgency: u8 = if message.severity == MessageSeverity::Critical {
            2
        } else {
            1
        };
        let hints = HashMap::from([
            ("desktop-entry", Value::from(DESKTOP_ENTRY)),
            ("urgency", Value::from(urgency)),
        ]);
        let reply = self
            .connection
            .call_method(
                Some(SERVER_NAME),
                SERVER_PATH,
                Some(SERVER_NAME),
                "Notify",
                &(
                    "PPVPN",
                    0u32,
                    DESKTOP_ENTRY,
                    message.title.as_str(),
                    body.as_str(),
                    vec!["default", open_label()],
                    hints,
                    -1i32,
                ),
            )
            .await?;
        let id: u32 = reply.body().deserialize()?;
        let mut tracked = self.tracked();
        tracked.shown.insert(id, message.id);
        tracked.trim();
        Ok(())
    }

    async fn has_body_markup(&self) -> zbus::Result<bool> {
        let reply = self
            .connection
            .call_method(
                Some(SERVER_NAME),
                SERVER_PATH,
                Some(SERVER_NAME),
                "GetCapabilities",
                &(),
            )
            .await?;
        let capabilities: Vec<String> = reply.body().deserialize()?;
        Ok(capabilities.iter().any(|name| name == "body-markup"))
    }

    fn open(&self, notification: u32) {
        let (push, token) = {
            let mut tracked = self.tracked();
            let Some(push) = tracked.shown.remove(&notification) else {
                return;
            };
            (push, tracked.activation_tokens.remove(&notification))
        };
        let mut command = Command::new(&self.app);
        command
            .arg("--open-notification")
            .arg(push.to_string())
            .stdin(Stdio::null());
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            command
                .env("XDG_ACTIVATION_TOKEN", &token)
                .env("DESKTOP_STARTUP_ID", &token);
        }
        match command.spawn() {
            // Reaped on a thread of its own: the app may run for long.
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => {
                tracing::warn!(
                    "push agent: could not start {}: {error}",
                    self.app.display()
                );
            }
        }
    }
}

/// With body-markup the server parses a subset of HTML: keep text literal.
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn open_label() -> &'static str {
    let env = |name: &str| std::env::var(name).ok();
    let language = env("LANGUAGE")
        .map(|value| value.split(':').next().unwrap_or_default().to_string())
        .or_else(|| env("LC_ALL"))
        .or_else(|| env("LC_MESSAGES"))
        .or_else(|| env("LANG"));
    if language.is_some_and(|language| language.starts_with("zh")) {
        "打开"
    } else {
        "Open"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markup_is_kept_literal() {
        assert_eq!(
            escape_markup("<b>a & b</b>"),
            "&lt;b&gt;a &amp; b&lt;/b&gt;"
        );
    }

    #[test]
    fn the_oldest_notifications_are_dropped_first() {
        let mut tracked = Tracked::default();
        for id in 1..=(MAX_TRACKED as u32 + 5) {
            tracked.shown.insert(id, u64::from(id));
            tracked.activation_tokens.insert(id, String::new());
        }
        tracked.trim();
        assert_eq!(tracked.shown.len(), MAX_TRACKED);
        assert!(!tracked.shown.contains_key(&5));
        assert!(tracked.shown.contains_key(&6));
        assert!(!tracked.activation_tokens.contains_key(&1));
    }
}
