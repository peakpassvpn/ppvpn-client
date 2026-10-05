//! Log lines as the Go core writes them (Go: internal/corelog), which the lab,
//! the netns CI and the performance checks parse (host-integration.md
//! section 10): `<RFC 3339 UTC, nanoseconds> level=<level> msg=<msg>
//! key=value ...`, a value quoted only when it is empty or holds a space,
//! tab, newline, `"` or `=`.

use std::fmt::{Display, Write as _};
use std::time::{SystemTime, UNIX_EPOCH};

/// One line, without its newline.
pub(crate) fn line(
    at: SystemTime,
    level: &str,
    msg: &str,
    fields: &[(&str, &dyn Display)],
) -> String {
    let mut out = String::with_capacity(96);
    out.push_str(&rfc3339_nano(at));
    out.push_str(" level=");
    out.push_str(level);
    out.push_str(" msg=");
    out.push_str(&quote(msg));
    for (key, value) in fields {
        let _ = write!(out, " {key}={}", quote(&value.to_string()));
    }
    out
}

/// As Go's corelog: bare unless empty or holding a space, tab, newline, `"`
/// or `=`; then a Go-style double-quoted string.
pub(crate) fn quote(s: &str) -> String {
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

/// Go's RFC3339Nano in UTC: `2006-01-02T15:04:05.999999999Z`, trailing zeros
/// of the fraction dropped, no fraction when it is zero.
pub(crate) fn rfc3339_nano(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    let mut s = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    );
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
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn lines_read_as_the_go_cores() {
        let at = UNIX_EPOCH + Duration::new(1_790_000_000, 120_000_000);
        let got = line(
            at,
            "info",
            "serve starting",
            &[("tun", &true), ("socket", &"/run/a b.sock"), ("empty", &"")],
        );
        assert_eq!(got, "2026-09-21T14:13:20.12Z level=info msg=\"serve starting\" tun=true socket=\"/run/a b.sock\" empty=\"\"");
        let got = line(
            UNIX_EPOCH + Duration::from_secs(951_782_400),
            "warn",
            "x",
            &[("k", &"a=b\"c\n")],
        );
        assert_eq!(
            got,
            "2000-02-29T00:00:00Z level=warn msg=x k=\"a=b\\\"c\\n\""
        );
    }
}
