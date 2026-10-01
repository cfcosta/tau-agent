//! Times as RFC 3339 in UTC, without a date library.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970, now.
pub fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `seconds` since 1970 as RFC 3339 UTC, e.g. `2026-09-29T12:00:00Z`.
pub fn rfc3339(seconds: u64) -> String {
    let rest = seconds % 86_400;
    format!(
        "{}T{:02}:{:02}:{:02}Z",
        date(seconds / 86_400),
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// The proleptic Gregorian date `days` after 1970-01-01, as
/// `YYYY-MM-DD` (Howard Hinnant's `civil_from_days`).
pub fn date(days: u64) -> String {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_as_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_790_032_532), "2026-09-21T23:15:32Z");
    }

    /// [`date`] agrees with counting the days off one year and
    /// one month at a time.
    #[hegel::test(test_cases = 500)]
    fn dates_match_counting_days(tc: hegel::TestCase) {
        use hegel::generators as gs;
        // Up to the year 2400 or so, past a century that is not a leap
        // year (2100) and one that is (2000).
        let days: u64 = tc.draw(gs::integers::<u64>().max_value(157_000));
        let leap = |year: u64| {
            (year.is_multiple_of(4) && !year.is_multiple_of(100))
                || year.is_multiple_of(400)
        };
        let (mut year, mut left) = (1970, days);
        while left >= if leap(year) { 366 } else { 365 } {
            left -= if leap(year) { 366 } else { 365 };
            year += 1;
        }
        let lengths = [
            31,
            if leap(year) { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        let mut month = 0;
        while left >= lengths[month] {
            left -= lengths[month];
            month += 1;
        }
        assert_eq!(
            date(days),
            format!("{year:04}-{:02}-{:02}", month + 1, left + 1)
        );
    }
}
