//! CSV export of analysis results (RFC 4180).
//!
//! Text cells that a spreadsheet would interpret as a formula (starting with
//! `=`, `+`, `-`, `@`, tab or carriage return) are prefixed with an
//! apostrophe. Numeric columns are written as plain numbers.

use crate::model::TrackRow;
use std::fmt::Write as _;

pub const COLUMNS: &[&str] = &[
    "path", "file_name", "title", "artist", "album", "format", "codec", "container", "lossless", "sample_rate_hz", "bit_depth", "bitrate_kbps", "channels",
    "duration_s", "size_bytes", "status", "overall", "dj", "listening", "confidence", "confidence_label", "verdict", "flags", "integrated_lufs",
    "true_peak_dbtp", "cutoff_khz", "analyzed_at_utc", "error",
];

fn cell(out: &mut String, s: &str, text: bool) {
    let guard = text && s.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let needs_quotes = guard || s.contains([',', '"', '\n', '\r']) || s.starts_with(' ') || s.ends_with(' ');
    if needs_quotes {
        out.push('"');
        if guard {
            out.push('\'');
        }
        for ch in s.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
    } else {
        out.push_str(s);
    }
}

fn opt_num(v: Option<f64>, d: usize) -> String {
    v.filter(|x| x.is_finite()).map(|x| format!("{x:.d$}")).unwrap_or_default()
}

/// ISO 8601 UTC timestamp from milliseconds since the epoch.
pub fn iso_utc(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem / 60) % 60, rem % 60)
}

pub fn header() -> String {
    let mut s = COLUMNS.join(",");
    s.push_str("\r\n");
    s
}

pub fn row(t: &TrackRow) -> String {
    let mut o = String::with_capacity(256);
    let text = |o: &mut String, s: &str| {
        cell(o, s, true);
        o.push(',');
    };
    let raw = |o: &mut String, s: &str| {
        cell(o, s, false);
        o.push(',');
    };
    text(&mut o, &t.path);
    text(&mut o, &t.file_name);
    text(&mut o, &t.display_title());
    text(&mut o, t.artist.as_deref().unwrap_or(""));
    text(&mut o, t.album.as_deref().unwrap_or(""));
    raw(&mut o, t.format.as_deref().unwrap_or(""));
    raw(&mut o, t.codec_name.as_deref().unwrap_or(""));
    raw(&mut o, t.container.as_deref().unwrap_or(""));
    raw(&mut o, t.lossless.map(|b| if b { "yes" } else { "no" }).unwrap_or(""));
    raw(&mut o, &t.sample_rate.map(|v| v.to_string()).unwrap_or_default());
    raw(&mut o, &t.bit_depth.map(|v| v.to_string()).unwrap_or_default());
    raw(&mut o, &opt_num(t.bitrate_kbps, 0));
    raw(&mut o, &t.channels.map(|v| v.to_string()).unwrap_or_default());
    raw(&mut o, &opt_num(t.duration, 2));
    raw(&mut o, &t.size.to_string());
    raw(&mut o, if t.missing_since.is_some() { "missing" } else { &t.status });
    raw(&mut o, &opt_num(t.overall, 0));
    raw(&mut o, &opt_num(t.dj, 0));
    raw(&mut o, &opt_num(t.listening, 0));
    raw(&mut o, &opt_num(t.confidence, 2));
    raw(&mut o, t.confidence_label.as_deref().unwrap_or(""));
    text(&mut o, t.verdict.as_deref().unwrap_or(""));
    raw(&mut o, &t.flags.join(" "));
    raw(&mut o, &opt_num(t.integrated_lufs, 1));
    raw(&mut o, &opt_num(t.true_peak, 2));
    raw(&mut o, &opt_num(t.cutoff_hz.map(|v| v / 1000.0), 1));
    raw(&mut o, &t.analyzed_at.map(iso_utc).unwrap_or_default());
    cell(&mut o, t.error_message.as_deref().unwrap_or(""), true);
    o.push_str("\r\n");
    o
}

pub fn to_csv(rows: &[TrackRow]) -> String {
    let mut s = header();
    for r in rows {
        let _ = write!(s, "{}", row(r));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_and_formula_guard() {
        let t = TrackRow {
            path: "/music/a, \"b\".flac".into(),
            file_name: "=HYPERLINK(\"x\").flac".into(),
            title: Some("-Intro".into()),
            integrated_lufs: Some(-9.73),
            status: "done".into(),
            ..Default::default()
        };
        let line = row(&t);
        assert!(line.starts_with("\"/music/a, \"\"b\"\".flac\","));
        assert!(line.contains("\"'=HYPERLINK(\"\"x\"\").flac\""));
        assert!(line.contains("\"'-Intro\""));
        assert!(line.contains(",-9.7,"), "numbers are not guarded: {line}");
        assert!(line.ends_with("\r\n"));
        assert_eq!(header().trim_end().split(',').count(), COLUMNS.len());
        assert!(line.trim_end().matches(',').count() >= COLUMNS.len() - 1);
    }

    #[test]
    fn iso_dates() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_760_875_200_000), "2025-10-19T12:00:00Z");
        assert_eq!(iso_utc(951_782_400_000), "2000-02-29T00:00:00Z");
    }
}
