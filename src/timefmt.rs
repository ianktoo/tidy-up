//! Tiny UTC time formatting helpers (avoids pulling in a date/time crate).
//!
//! # Why dates here are UTC
//!
//! The standard library has no time-zone API, and there is no portable way to
//! read the local offset without a dependency: it would mean a TZif parser plus
//! `/etc/localtime` on Unix and a registry read on Windows. Rather than get it
//! right on one platform and silently wrong on another, everything here is UTC,
//! and [`utc_offset`] lets a user say otherwise.
//!
//! This matters for `reorganize --by year` or `--by month`, where a file
//! modified late in the evening can land in the previous day, month or year.
//! Set `TIDY_UP_UTC_OFFSET` to something like `+03:00` to correct it.

use std::time::{SystemTime, UNIX_EPOCH};

/// Environment variable holding the offset to apply to every displayed and
/// grouped date, as `+HH:MM`, `-HH:MM`, `+HH` or `0`.
pub const OFFSET_VAR: &str = "TIDY_UP_UTC_OFFSET";

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

/// The offset to apply to timestamps, in seconds east of UTC.
///
/// Reads [`OFFSET_VAR`]; anything unset or unparseable means UTC, because a
/// guess would be worse than a stated default.
pub fn utc_offset() -> i64 {
    std::env::var(OFFSET_VAR)
        .ok()
        .as_deref()
        .and_then(parse_offset)
        .unwrap_or(0)
}

/// Parses `+03:00`, `-0500`, `+7`, `Z` or `0` into seconds east of UTC.
pub fn parse_offset(text: &str) -> Option<i64> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("z") || text.eq_ignore_ascii_case("utc") {
        return Some(0);
    }
    let (sign, rest) = match text.as_bytes()[0] {
        b'-' => (-1, &text[1..]),
        b'+' => (1, &text[1..]),
        _ => (1, text),
    };
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    let (hours, minutes) = match digits.len() {
        1 | 2 => (digits.parse::<i64>().ok()?, 0),
        3 => (digits[..1].parse().ok()?, digits[1..].parse().ok()?),
        4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?),
        _ => return None,
    };
    // Real offsets run from -12:00 to +14:00; anything else is a typo.
    (hours <= 14 && minutes < 60).then_some(sign * (hours * 3600 + minutes * 60))
}

/// The `(year, month, day)` a timestamp falls on, once `offset` is applied.
///
/// Used to group files into date folders. A timestamp before the epoch, which
/// `offset` can produce, clamps to the epoch rather than panicking.
pub fn civil_date(secs: u64, offset: i64) -> (i64, u32, u32) {
    let shifted = (secs as i64).saturating_add(offset).max(0);
    civil_from_days(shifted.div_euclid(86_400))
}

/// English month names, indexed from 1.
pub const MONTHS: [&str; 13] = [
    "",
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

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

/// `20260920-140309`: sortable and filename-safe.
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

    #[test]
    fn offsets_parse_in_every_shape_people_write_them() {
        assert_eq!(parse_offset("+03:00"), Some(3 * 3600));
        assert_eq!(parse_offset("+0300"), Some(3 * 3600));
        assert_eq!(parse_offset("+3"), Some(3 * 3600));
        assert_eq!(parse_offset("3"), Some(3 * 3600));
        assert_eq!(parse_offset("-05:30"), Some(-(5 * 3600 + 30 * 60)));
        assert_eq!(parse_offset("-0530"), Some(-(5 * 3600 + 30 * 60)));
        assert_eq!(parse_offset("+05:45"), Some(5 * 3600 + 45 * 60), "Nepal");
        assert_eq!(parse_offset("Z"), Some(0));
        assert_eq!(parse_offset(""), Some(0));
    }

    #[test]
    fn nonsense_offsets_are_rejected_rather_than_guessed() {
        assert_eq!(parse_offset("+25:00"), None, "no such offset exists");
        assert_eq!(parse_offset("+03:99"), None);
        assert_eq!(parse_offset("Nairobi"), None);
        assert_eq!(parse_offset("+123456"), None);
    }

    /// The reason the offset exists: just before midnight, UTC and local time
    /// disagree about the date, and for `--by year` they disagree about the year.
    #[test]
    fn an_offset_moves_a_late_evening_file_into_the_right_day() {
        // 2023-12-31 22:30:00 UTC
        let new_years_eve = 1_704_061_800;
        assert_eq!(civil_date(new_years_eve, 0), (2023, 12, 31));
        assert_eq!(
            civil_date(new_years_eve, 3 * 3600),
            (2024, 1, 1),
            "in UTC+3 that moment is already the new year"
        );
    }

    #[test]
    fn a_negative_offset_before_the_epoch_clamps_instead_of_panicking() {
        assert_eq!(civil_date(0, -12 * 3600), (1970, 1, 1));
    }

    #[test]
    fn month_names_line_up_with_their_numbers() {
        assert_eq!(MONTHS[1], "January");
        assert_eq!(MONTHS[12], "December");
        assert_eq!(MONTHS.len(), 13, "index 0 is a deliberate blank");
    }
}
