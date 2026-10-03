//! serve's diagnostic log, line for line as the Go core writes it (Go:
//! internal/corelog): `<RFC 3339 UTC, nanoseconds> level=<level> msg=<msg>
//! key=value ...`, values quoted only when they hold a space, tab, newline,
//! `"` or `=`, or are empty. The lab, the netns CI and the performance checks
//! parse these lines (host-integration.md section 10).

use std::fmt::{Display, Write as _};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

pub const LEVEL_INFO: &str = "info";
pub const LEVEL_DEBUG: &str = "debug";

/// What one line is written to.
enum Sink {
    Stderr,
    File(File),
    Lines(Box<dyn Fn(&str) + Send + Sync>),
    Discard,
}

/// A logger: cheap to clone, every clone writes to the same place.
#[derive(Clone)]
pub struct Logger(Arc<Inner>);

struct Inner {
    sink: Mutex<Sink>,
    debug: AtomicBool,
}

impl Logger {
    fn with(sink: Sink) -> Self {
        Logger(Arc::new(Inner { sink: Mutex::new(sink), debug: AtomicBool::new(false) }))
    }

    /// To standard error.
    pub fn stderr() -> Self {
        Self::with(Sink::Stderr)
    }

    /// Appended to `path`, created private to this account (0600).
    pub fn open_file(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        Ok(Self::with(Sink::File(options.open(path)?)))
    }

    /// Each line, without its newline, to `f` (tests, and hosts that take
    /// lines themselves).
    pub fn lines(f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self::with(Sink::Lines(Box::new(f)))
    }

    pub fn discard() -> Self {
        Self::with(Sink::Discard)
    }

    /// `info` or `debug`; anything else is an error, as in the Go core.
    pub fn set_level(&self, level: &str) -> Result<(), String> {
        match level {
            LEVEL_INFO => self.0.debug.store(false, Ordering::Relaxed),
            LEVEL_DEBUG => self.0.debug.store(true, Ordering::Relaxed),
            _ => return Err(format!("unknown log level {level:?} (want info or debug)")),
        }
        Ok(())
    }

    pub fn debug_enabled(&self) -> bool {
        self.0.debug.load(Ordering::Relaxed)
    }

    /// The level name: `info` or `debug`.
    pub fn level(&self) -> &'static str {
        if self.debug_enabled() {
            LEVEL_DEBUG
        } else {
            LEVEL_INFO
        }
    }

    pub fn info(&self, msg: &str, fields: &[(&str, &dyn Display)]) {
        self.write("info", msg, fields, true);
    }

    pub fn warn(&self, msg: &str, fields: &[(&str, &dyn Display)]) {
        self.write("warn", msg, fields, true);
    }

    pub fn error(&self, msg: &str, fields: &[(&str, &dyn Display)]) {
        self.write("error", msg, fields, true);
    }

    pub fn debug(&self, msg: &str, fields: &[(&str, &dyn Display)]) {
        if self.debug_enabled() {
            self.write("debug", msg, fields, false);
        }
    }

    /// A line formatted already (the engine's, from its log channel).
    pub fn raw(&self, line: &str) {
        let mut line = redact_text(line.trim_end_matches('\n'));
        line.push('\n');
        self.emit(&line, true);
    }

    fn write(&self, level: &str, msg: &str, fields: &[(&str, &dyn Display)], flush: bool) {
        let line = redact_text(&format_line(SystemTime::now(), level, msg, fields));
        self.emit(&line, flush);
    }

    fn emit(&self, line: &str, flush: bool) {
        let mut sink = self.0.sink.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *sink {
            Sink::Stderr => {
                let _ = io::stderr().lock().write_all(line.as_bytes());
            }
            Sink::File(file) => {
                let _ = file.write_all(line.as_bytes());
                if flush {
                    let _ = file.sync_data();
                }
            }
            Sink::Lines(f) => f(line.trim_end_matches('\n')),
            Sink::Discard => {}
        }
    }
}

/// One line, newline included.
pub fn format_line(at: SystemTime, level: &str, msg: &str, fields: &[(&str, &dyn Display)]) -> String {
    let mut line = String::with_capacity(96);
    line.push_str(&rfc3339_nano(at));
    line.push_str(" level=");
    line.push_str(level);
    line.push_str(" msg=");
    line.push_str(&quote(msg));
    for (key, value) in fields {
        let _ = write!(line, " {key}={}", quote(&value.to_string()));
    }
    line.push('\n');
    line
}

/// As Go's corelog: bare unless empty or holding a space, tab, newline,
/// `"` or `=`; then a Go-style double-quoted string.
pub fn quote(s: &str) -> String {
    if !s.is_empty() && !s.contains([' ', '\t', '\n', '"', '=']) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Hides the credentials of proxy URLs (`http://user:pass@host` and the
/// like), as the Go core's `redact.Text`.
pub fn redact_text(s: &str) -> String {
    const HIDDEN: &str = "[REDACTED]";
    let lower = s.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let rest = &lower[i..];
        let scheme = ["https://", "http://", "socks5://"].into_iter().find(|p| rest.starts_with(p));
        if let Some(scheme) = scheme {
            let start = i + scheme.len();
            let end = s[start..].find(|c: char| c == '/' || c == '@' || c.is_whitespace()).map(|n| start + n);
            if let Some(at) = end.filter(|&n| n > start && s.as_bytes()[n] == b'@') {
                out.push_str(&s[i..start]);
                out.push_str(HIDDEN);
                out.push('@');
                i = at + 1;
                continue;
            }
            out.push_str(&s[i..start]);
            i = start;
            continue;
        }
        let c = s[i..].chars().next().expect("char");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `2006-01-02T15:04:05.999999999Z`: Go's RFC3339Nano in UTC (trailing
/// zeros of the fraction dropped, no fraction when it is zero).
pub fn rfc3339_nano(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let mut s = format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}", rem / 3600, rem / 60 % 60, rem % 60);
    let nanos = since.subsec_nanos();
    if nanos != 0 {
        let fraction = format!("{nanos:09}");
        s.push('.');
        s.push_str(fraction.trim_end_matches('0'));
    }
    s.push('Z');
    s
}

/// Days since 1970-01-01 to (year, month, day), proleptic Gregorian.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lines_read_as_the_go_cores() {
        let at = UNIX_EPOCH + Duration::new(1_790_000_000, 120_000_000);
        let line = format_line(at, "info", "serve starting", &[("tun", &true), ("socket", &"/run/a b.sock"), ("empty", &"")]);
        assert_eq!(line, "2026-09-21T14:13:20.12Z level=info msg=\"serve starting\" tun=true socket=\"/run/a b.sock\" empty=\"\"\n");
        let line = format_line(UNIX_EPOCH + Duration::from_secs(951_782_400), "warn", "x", &[("k", &"a=b\"c\n")]);
        assert_eq!(line, "2000-02-29T00:00:00Z level=warn msg=x k=\"a=b\\\"c\\n\"\n");
    }

    #[test]
    fn levels() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let got = lines.clone();
        let log = Logger::lines(move |l| got.lock().unwrap().push(l.split_once(' ').unwrap().1.to_string()));
        log.debug("hidden", &[]);
        log.info("shown", &[]);
        assert!(log.set_level("trace").is_err());
        log.set_level("debug").unwrap();
        log.debug("now shown", &[("n", &1)]);
        assert_eq!(*lines.lock().unwrap(), ["level=info msg=shown", "level=debug msg=\"now shown\" n=1"]);
    }

    #[test]
    fn proxy_credentials_are_hidden() {
        assert_eq!(
            redact_text("via socks5://u:p@127.0.0.1:7890 and HTTP://x@h/ and http://h/p@q"),
            "via socks5://[REDACTED]@127.0.0.1:7890 and HTTP://[REDACTED]@h/ and http://h/p@q"
        );
    }
}
