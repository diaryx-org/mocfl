//! The clock the library refuses to have.
//!
//! `mocfl` takes every timestamp from its caller so it stays deterministic and
//! dependency-free. Something has to actually read a clock, though, and for a
//! command-line run that something is here.
//!
//! Formatting a Unix instant as RFC 3339 is a closed, well-specified problem —
//! the civil-from-days algorithm below is exact for the whole proleptic
//! Gregorian range — so it is implemented rather than pulled in, on the same
//! reasoning as the crate's hand-rolled SHA-256. A date library would be the
//! single largest dependency in this workspace, and it would earn that weight
//! only if this needed parsing, zones, or locales. It needs none of them: OCFL
//! wants UTC, to the second, with a `Z`.

use std::time::{SystemTime, UNIX_EPOCH};

/// The current instant as an OCFL-shaped RFC 3339 timestamp (`2026-07-27T09:14:00Z`).
pub fn now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        // A clock set before 1970 is absurd but not worth a panic; the epoch is
        // a visibly wrong timestamp rather than a crash mid-commit.
        .unwrap_or(0);
    format_rfc3339(secs)
}

/// Format Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_rfc3339(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Days since 1970-01-01 → `(year, month, day)` in the proleptic Gregorian
/// calendar. Howard Hinnant's `civil_from_days`, which is exact and branch-light
/// because it counts from March (so the leap day lands at the end of a year).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March = 0
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (mp + if mp < 10 { 3 } else { -9 }) as u32; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_instants() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(86_399), "1970-01-01T23:59:59Z");
        assert_eq!(format_rfc3339(86_400), "1970-01-02T00:00:00Z");
        assert_eq!(format_rfc3339(1_000_000_000), "2001-09-09T01:46:40Z");
        assert_eq!(format_rfc3339(1_234_567_890), "2009-02-13T23:31:30Z");
    }

    #[test]
    fn handles_leap_days_and_century_rules() {
        // 2000 is a leap year (divisible by 400); 1900 was not (divisible by 100).
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_rfc3339(-2_203_891_200), "1900-03-01T00:00:00Z");
    }

    #[test]
    fn a_pre_epoch_instant_does_not_wrap() {
        // Euclidean division, not truncating: one second before the epoch is the
        // last second of 1969, never a negative hour.
        assert_eq!(format_rfc3339(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn the_shape_is_what_ocfl_asks_for() {
        // UTC, seconds precision, explicit offset. `rocfl` warns about anything
        // less, and the spec requires a timezone.
        let stamp = now();
        assert_eq!(stamp.len(), 20, "{stamp}");
        assert!(stamp.ends_with('Z'), "{stamp}");
        assert_eq!(stamp.as_bytes()[10], b'T', "{stamp}");
    }
}
