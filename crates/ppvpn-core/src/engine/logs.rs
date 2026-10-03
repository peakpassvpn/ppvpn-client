//! An instance's log (docs/host-integration.md, section 10): the Engine's
//! own `tracing` events and sail's lines, as logfmt, into one bounded queue
//! per instance, and from it to the instance's `LogSink`.
//!
//! - The process's subscriber: [`install`] tries once to set the global
//!   default to sail's layer plus [`tracing_layer`]; a host that installed
//!   its own adds both to it.
//! - Which instance a line is for: an event's `instance` field, else the
//!   closest enclosing span with one ([`Logs::span`]). An event with
//!   neither goes to every instance.
//! - Never waiting: lines that do not fit the queue (or reach no reader)
//!   are dropped and counted; the next line that fits is preceded by a warn
//!   line with how many went.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write as _};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock, RwLock, Weak};
use std::time::SystemTime;

use tokio::sync::mpsc;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, Layer, SubscriberExt as _};
use tracing_subscriber::registry::LookupSpan;

use crate::config::{LogConfig, LogLevel, LogSink};
use crate::error::{codes, Error};
use crate::event::LogReceiver;
use crate::logfmt;
use crate::runtime::Runtime;

/// Lines an instance holds for its sink.
pub(super) const QUEUE: usize = 1024;

/// The field (on an event or a span) that names the instance a line is for.
const INSTANCE_FIELD: &str = "instance";

/// The live instances' pipes, for the layer. Dead entries go at the next
/// registration.
static PIPES: RwLock<Vec<Weak<Pipe>>> = RwLock::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Sets the process's subscriber, once: sail's layer (each sail instance's
/// lines to its `Instance::logs`) and ours. A host's own subscriber, set
/// before, stays; the host then adds both layers itself (section 10).
pub(crate) fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let subscriber = tracing_subscriber::registry()
            .with(sail::embed::tracing_layer())
            .with(tracing_layer());
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

/// The layer that turns ppvpn-core's own events (targets `ppvpn_core` and
/// `ppvpn_core::…`) into the log lines of the instances they are for. A
/// host that installs its own `tracing` subscriber adds it, with
/// `sail::embed::tracing_layer()`; without them no line reaches the
/// instances' sinks (section 10).
pub fn tracing_layer<S>() -> impl Layer<S> + Send + Sync + 'static
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    CoreLayer.with_filter(filter_fn(|metadata| {
        is_core(metadata.target()) && *metadata.level() <= Level::DEBUG
    }))
}

fn is_core(target: &str) -> bool {
    target == "ppvpn_core" || target.starts_with("ppvpn_core::")
}

/// An instance's log: its pipe, and the channel end `Engine::logs` hands
/// out once.
pub(super) struct Logs {
    pipe: Arc<Pipe>,
    receiver: Mutex<Option<mpsc::Receiver<String>>>,
}

impl Logs {
    /// The log `config` asks for; a `File` sink's file is opened (created,
    /// appended to) here.
    pub(super) fn new(config: &LogConfig) -> Result<Logs, Error> {
        let (tx, receiver) = match &config.sink {
            LogSink::File { path } => {
                let file = open(path).map_err(|e| {
                    Error::invalid(
                        codes::CORE_OPERATION_FAILED,
                        "log.sink.path",
                        format!("log: cannot open the log file: {e}"),
                    )
                })?;
                let (tx, rx) = mpsc::channel(QUEUE);
                (Some(tx), Some((rx, Some(file))))
            }
            LogSink::Channel => {
                let (tx, rx) = mpsc::channel(QUEUE);
                (Some(tx), Some((rx, None)))
            }
            _ => (None, None),
        };
        let pipe = Arc::new(Pipe {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            level: match config.level {
                LogLevel::Debug => Level::DEBUG,
                _ => Level::INFO,
            },
            tx,
            dropped: Arc::new(AtomicU64::new(0)),
            reported: AtomicU64::new(0),
            runtime: OnceLock::new(),
        });
        let receiver = match receiver {
            Some((rx, Some(file))) => {
                spawn_writer(rx, file, pipe.dropped.clone()).map_err(|e| {
                    Error::new(
                        codes::CORE_OPERATION_FAILED,
                        false,
                        format!("log: cannot start the writer thread: {e}"),
                    )
                })?;
                None
            }
            Some((rx, None)) => Some(rx),
            None => None,
        };
        let mut pipes = PIPES.write().unwrap_or_else(|e| e.into_inner());
        pipes.retain(|p| p.strong_count() > 0);
        pipes.push(Arc::downgrade(&pipe));
        drop(pipes);
        Ok(Logs {
            pipe,
            receiver: Mutex::new(receiver),
        })
    }

    /// A log that keeps nothing: what an instance falls back to when its
    /// sink cannot be opened where no error can be returned.
    pub(super) fn discard() -> Logs {
        Logs::new(&LogConfig::default()).expect("a log without a sink")
    }

    /// Sends `runtime`'s lines (sail's) into this log, from a thread of its
    /// own until the runtime closes them. Once per instance.
    pub(super) fn attach(&self, runtime: &Arc<dyn Runtime>) {
        let _ = self.pipe.runtime.set(Arc::downgrade(runtime));
        let mut lines = runtime.logs();
        let pipe = Arc::downgrade(&self.pipe);
        let spawned = std::thread::Builder::new()
            .name("ppvpn-core-logs".into())
            .spawn(move || {
                while let Some(line) = lines.blocking_recv() {
                    let Some(pipe) = pipe.upgrade() else { return };
                    pipe.push(line);
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(error = %e, "log: cannot start the thread for sail's lines");
        }
    }

    /// The span whose events are this instance's alone.
    pub(super) fn span(&self) -> tracing::Span {
        tracing::info_span!("instance", instance = self.pipe.id)
    }

    /// The lines, with a `Channel` sink, once; afterwards (and with any
    /// other sink) a receiver that is already closed.
    pub(super) fn receiver(&self) -> LogReceiver {
        let receiver = self
            .receiver
            .lock()
            .expect("log receiver")
            .take()
            .unwrap_or_else(|| mpsc::channel(1).1);
        LogReceiver { receiver }
    }

    /// Lines dropped so far, sail's included (`Status::dropped_log_lines`).
    pub(super) fn dropped(&self) -> u64 {
        self.pipe.dropped()
    }
}

/// Where an instance's lines go in; shared with the layer and the threads.
struct Pipe {
    id: u64,
    /// The most verbose level the instance keeps.
    level: Level,
    /// None: `LogSink::None`, nothing is kept.
    tx: Option<mpsc::Sender<String>>,
    dropped: Arc<AtomicU64>,
    /// Of `dropped()`, how many the warn lines have told of.
    reported: AtomicU64,
    runtime: OnceLock<Weak<dyn Runtime>>,
}

impl Pipe {
    fn takes(&self, level: &Level) -> bool {
        self.tx.is_some() && level <= &self.level
    }

    fn dropped(&self) -> u64 {
        let runtime = self
            .runtime
            .get()
            .and_then(Weak::upgrade)
            .map_or(0, |r| r.dropped_log_lines());
        self.dropped.load(Ordering::Relaxed) + runtime
    }

    /// Queues `line` without waiting: dropped and counted when it does not
    /// fit. After drops, the first line that fits is preceded by a warn line
    /// with their number.
    fn push(&self, line: String) {
        let Some(tx) = &self.tx else { return };
        let reported = self.reported.load(Ordering::Relaxed);
        let unreported = self.dropped().saturating_sub(reported);
        if unreported > 0
            && self
                .reported
                .compare_exchange(
                    reported,
                    reported + unreported,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .is_ok()
        {
            let summary = logfmt::line(
                SystemTime::now(),
                "warn",
                "log lines dropped",
                &[("dropped", &unreported), ("source", &"core")],
            );
            if tx.try_send(summary).is_err() {
                self.reported.fetch_sub(unreported, Ordering::Relaxed);
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        if tx.try_send(line).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Appends, creating the file (0600 on Unix) when it is missing.
fn open(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

/// Writes the queue to `file` from a thread of its own, so no writer of a
/// line waits on the disk; it ends once the instance is gone. A line that
/// cannot be written counts as dropped.
fn spawn_writer(
    mut lines: mpsc::Receiver<String>,
    file: File,
    dropped: Arc<AtomicU64>,
) -> io::Result<()> {
    std::thread::Builder::new()
        .name("ppvpn-core-log-file".into())
        .spawn(move || {
            let mut out = BufWriter::new(file);
            while let Some(line) = lines.blocking_recv() {
                if writeln!(out, "{line}").is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
                if lines.is_empty() {
                    let _ = out.flush();
                }
            }
            let _ = out.flush();
        })
        .map(drop)
}

/// The instance a span names, kept in the span's extensions.
struct Instance(u64);

struct CoreLayer;

impl<S> Layer<S> for CoreLayer
where
    S: Subscriber + for<'span> LookupSpan<'span>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let mut fields = Fields::default();
        attrs.record(&mut fields);
        if let (Some(instance), Some(span)) = (fields.instance, ctx.span(id)) {
            span.extensions_mut().insert(Instance(instance));
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let level = event.metadata().level();
        let mut fields = Fields::default();
        event.record(&mut fields);
        let instance = fields.instance.or_else(|| {
            ctx.event_scope(event)?
                .find_map(|span| span.extensions().get::<Instance>().map(|i| i.0))
        });
        // Out of the lock before a push: a pipe may drop with its last
        // handle here.
        let pipes: Vec<Arc<Pipe>> = PIPES
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|p| p.takes(level) && instance.is_none_or(|i| i == p.id))
            .collect();
        if pipes.is_empty() {
            return;
        }
        let pairs: Vec<(&str, &dyn std::fmt::Display)> = fields
            .pairs
            .iter()
            .map(|(k, v)| (*k, v as &dyn std::fmt::Display))
            .chain([("source", &"core" as &dyn std::fmt::Display)])
            .collect();
        let line = logfmt::line(
            SystemTime::now(),
            &level.as_str().to_ascii_lowercase(),
            &fields.message,
            &pairs,
        );
        for pipe in pipes {
            pipe.push(line.clone());
        }
    }
}

/// An event's message and fields, as text; `instance` apart.
#[derive(Default)]
struct Fields {
    message: String,
    pairs: Vec<(&'static str, String)>,
    instance: Option<u64>,
}

impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == INSTANCE_FIELD {
            self.instance = Some(value);
        } else {
            self.pairs.push((field.name(), value.to_string()));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.pairs.push((field.name(), value.to_owned()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.pairs.push((field.name(), format!("{value:?}")));
        }
    }
}

#[cfg(test)]
#[path = "logs_tests.rs"]
mod tests;
