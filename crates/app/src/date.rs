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
}
