//! Minimal unix-timestamp -> calendar-date formatting, just enough for
//! "created Jan 3, 2026" labels. No `chrono` dependency: CLAUDE.md asks to
//! keep dependencies minimal, and this is the only place that needs dates.

/// Converts days since the Unix epoch to a (year, month, day) civil date.
/// Howard Hinnant's `civil_from_days` algorithm (public domain), proleptic
/// Gregorian calendar.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Formats a unix timestamp (seconds) as e.g. "Jan 3, 2026".
pub fn format_date(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let month_name = MONTHS
        .get((month as usize).saturating_sub(1))
        .unwrap_or(&"?");
    format!("{month_name} {day}, {year}")
}

/// Inverse of [`civil_from_days`]: days since the Unix epoch for a civil
/// date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

/// Parses `YYYY-MM-DD` into a unix timestamp at 00:00 UTC, or `None` if it
/// isn't a real calendar date (rejects 2026-02-30, month 13, junk).
pub fn parse_date(text: &str) -> Option<i64> {
    let mut parts = text.trim().split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !(1..=9999).contains(&year) || !(1..=12).contains(&month) {
        return None;
    }
    let days_in_month = match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&day) {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400)
}

/// Formats a unix timestamp as `YYYY-MM-DD`, the form [`parse_date`] reads.
pub fn format_iso_date(unix_seconds: i64) -> String {
    let (year, month, day) = civil_from_days(unix_seconds.div_euclid(86_400));
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_epoch() {
        assert_eq!(format_date(0), "Jan 1, 1970");
    }

    #[test]
    fn formats_known_date() {
        // 2024-01-01 00:00:00 UTC
        assert_eq!(format_date(1_704_067_200), "Jan 1, 2024");
    }

    #[test]
    fn parses_iso_dates_and_round_trips() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2024-01-01"), Some(1_704_067_200));
        assert_eq!(parse_date("  2026-10-30 "), Some(1_793_318_400));
        for ts in [
            0,
            1_704_067_200,
            1_793_318_400,
            951_782_400, /* 2000-02-29 */
        ] {
            assert_eq!(parse_date(&format_iso_date(ts)), Some(ts));
        }
        // Dates before the epoch work too.
        assert_eq!(parse_date("1969-12-31"), Some(-86_400));
    }

    #[test]
    fn rejects_impossible_dates() {
        for bad in [
            "",
            "tomorrow",
            "2026-13-01",
            "2026-00-10",
            "2026-02-30",
            "2025-02-29",
            "2026-04-31",
            "2026-1-",
            "2026-01-01-01",
            "0-01-01",
            "2026-01-00",
        ] {
            assert_eq!(parse_date(bad), None, "{bad:?} should be rejected");
        }
        assert!(parse_date("2024-02-29").is_some(), "leap day is valid");
        assert!(parse_date("2000-02-29").is_some(), "2000 is a leap year");
        assert!(parse_date("1900-02-29").is_none(), "1900 is not");
    }
}
