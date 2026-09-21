//! Tiny UTC time formatting helpers (avoids pulling in a date/time crate).

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch (0 if the clock is before 1970).
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Converts days since 1970-01-01 to a `(year, month, day)` civil date.
///
/// This is the well-known `civil_from_days` algorithm by Howard Hinnant.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn parts(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    let (y, m, d) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `2026-09-20 14:03:09 UTC`
pub fn format_utc(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = parts(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// `20260920-140309` — sortable and filename-safe.
pub fn compact_id(secs: u64) -> String {
    let (y, mo, d, h, mi, s) = parts(secs);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00 UTC");
    }

    #[test]
    fn known_timestamp() {
        assert_eq!(format_utc(1_700_000_000), "2023-11-14 22:13:20 UTC");
        assert_eq!(compact_id(1_700_000_000), "20231114-221320");
    }

    #[test]
    fn leap_day() {
        assert_eq!(format_utc(1_709_208_000), "2024-02-29 12:00:00 UTC");
    }

    #[test]
    fn ids_sort_chronologically() {
        assert!(compact_id(1_000_000_000) < compact_id(1_700_000_000));
    }
}
