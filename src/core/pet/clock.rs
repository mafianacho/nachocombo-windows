//! Time for the pet without a date crate: Unix seconds as `f64`, RFC 3339 parsing and civil dates. The local UTC offset
//! is set by the shell (Windows: `GetTimeZoneInformation`) — the core never asks the OS, so tests pin it.
use std::sync::atomic::{AtomicI64, Ordering};

static LOCAL_OFFSET: AtomicI64 = AtomicI64::new(9 * 3600);

/// Seconds since 1970-01-01 UTC.
pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The machine's offset from UTC in seconds (default +09:00 until the shell sets it).
pub fn local_offset() -> i64 { LOCAL_OFFSET.load(Ordering::Relaxed) }
pub fn set_local_offset(secs: i64) { LOCAL_OFFSET.store(secs, Ordering::Relaxed) }

/// Days since 1970-01-01 → (year, month 1-12, day 1-31). Howard Hinnant's civil_from_days.
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (year, month, day) → days since 1970-01-01.
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `2026-11-14T09:30:00Z`, `…+09:00`, with or without fractional seconds → Unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || !(b[10] == b'T' || b[10] == b't' || b[10] == b' ') || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        if t.bytes().all(|c| c.is_ascii_digit()) { t.parse().ok() } else { None }
    };
    let (y, mo, d, h, mi, se) = (num(0..4)?, num(5..7)?, num(8..10)?, num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 { return None; }
    let mut i = 19;
    let mut frac = 0.0;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() { i += 1; }
        if i == start { return None; }
        frac = format!("0.{}", &s[start..i]).parse().ok()?;
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' if i + 1 == b.len() => 0,
        c @ (b'+' | b'-') if b.len() == i + 6 && b[i + 3] == b':' => {
            let oh = num(i + 1..i + 3)?;
            let om = num(i + 4..i + 6)?;
            let o = oh * 3600 + om * 60;
            if *c == b'-' { -o } else { o }
        }
        _ => return None,
    };
    let days = days_from_civil(y, mo as u32, d as u32);
    Some((days * 86_400 + h * 3600 + mi * 60 + se - offset) as f64 + frac)
}

/// Local wall clock of `t` at `offset`: (year, month, day, hour, minute, weekday 0 = Sunday).
pub fn local_parts(t: f64, offset: i64) -> (i64, u32, u32, u32, u32, u32) {
    let secs = t.floor() as i64 + offset;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil(days);
    let wd = (days + 4).rem_euclid(7) as u32;
    (y, m, d, (rem / 3600) as u32, ((rem % 3600) / 60) as u32, wd)
}

/// Korean "오전 9:05" / "오후 3:40" (the macOS `a h:mm` pattern with the ko_KR locale).
pub fn ko_time(hour: u32, minute: u32) -> String {
    let ampm = if hour < 12 { "오전" } else { "오후" };
    let h = match hour % 12 { 0 => 12, h => h };
    format!("{ampm} {h}:{minute:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_and_civil_dates() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_rfc3339("2026-11-14T09:00:00+09:00"), parse_rfc3339("2026-11-14T00:00:00Z"));
        assert_eq!(parse_rfc3339("2026-11-14T00:00:00.500Z").map(|t| t.fract()), Some(0.5));
        assert_eq!(parse_rfc3339("2026-11-14 00:00:00Z"), parse_rfc3339("2026-11-14T00:00:00Z"));
        assert!(parse_rfc3339("tomorrow").is_none());
        assert!(parse_rfc3339("2026-13-14T00:00:00Z").is_none());
        assert!(parse_rfc3339("2026-11-14T00:00:00").is_none());
        for days in [-1000, 0, 365, 20_406, 50_000] { let (y, m, d) = civil(days); assert_eq!(days_from_civil(y, m, d), days); }
        let t = parse_rfc3339("2026-11-14T06:05:00Z").unwrap();
        assert_eq!(local_parts(t, 9 * 3600), (2026, 11, 14, 15, 5, 6));
        assert_eq!(ko_time(15, 5), "오후 3:05");
        assert_eq!(ko_time(0, 30), "오전 12:30");
        assert_eq!(ko_time(12, 0), "오후 12:00");
    }
}
