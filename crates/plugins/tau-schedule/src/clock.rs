//! The host's clock: what time it is, and the host's local days, which
//! an agent's schedules and its daily cap go by (ADR 0037), and what a
//! chat schedules for itself (ADR 0039); durations and times of day as
//! both write them.

use std::sync::atomic::{AtomicI64, Ordering};

/// What time it is, and how far the host's local time is from UTC.
pub trait Clock: Send + Sync {
    /// Seconds since the epoch.
    fn now(&self) -> i64;

    /// The host's offset from UTC at `at`, in seconds.
    fn offset(&self, at: i64) -> i32;
}

/// The machine's clock and time zone.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs() as i64)
    }

    #[allow(unsafe_code, reason = "libc's localtime_r, on a local buffer")]
    fn offset(&self, at: i64) -> i32 {
        let time = at as libc::time_t;
        // SAFETY: `localtime_r` only writes the `tm` it is given, and
        // reads `time`; both live on this stack frame.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        let local = unsafe { libc::localtime_r(&time, &mut tm) };
        if local.is_null() {
            0
        } else {
            tm.tm_gmtoff as i32
        }
    }
}

/// A clock a test sets: a fixed offset, and a time it moves itself.
#[derive(Debug, Default)]
pub struct FakeClock {
    now: AtomicI64,
    offset: i32,
}

impl FakeClock {
    pub fn new(now: i64, offset: i32) -> Self {
        Self {
            now: AtomicI64::new(now),
            offset,
        }
    }

    pub fn set(&self, now: i64) {
        self.now.store(now, Ordering::SeqCst);
    }

    pub fn advance(&self, seconds: i64) {
        self.now.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> i64 {
        self.now.load(Ordering::SeqCst)
    }

    fn offset(&self, _at: i64) -> i32 {
        self.offset
    }
}

const DAY: i64 = 86_400;

/// `text` as seconds: a number, then `s`, `m`, `h` or `d`.
pub fn duration(text: &str) -> Option<i64> {
    let text = text.trim();
    let unit = text.chars().last()?;
    let number: i64 = text[..text.len() - unit.len_utf8()].parse().ok()?;
    let scale = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => DAY,
        _ => return None,
    };
    (number > 0).then(|| number.checked_mul(scale)).flatten()
}

/// `HH:MM` as seconds into the day.
pub fn time_of_day(text: &str) -> Option<i64> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let (hours, minutes): (i64, i64) =
        (hours.parse().ok()?, minutes.parse().ok()?);
    ((0..24).contains(&hours) && (0..60).contains(&minutes))
        .then_some(hours * 3600 + minutes * 60)
}

/// The local day `at` falls on, counted from 1970-01-01.
fn local_days(at: i64, offset: i32) -> i64 {
    (at + i64::from(offset)).div_euclid(DAY)
}

/// The local day `at` falls on, as `YYYY-MM-DD`.
pub fn day(clock: &dyn Clock, at: i64) -> String {
    let (year, month, day) = civil(local_days(at, clock.offset(at)));
    format!("{year:04}-{month:02}-{day:02}")
}

/// When the local day `at` falls on began, in seconds since the epoch.
pub fn midnight(clock: &dyn Clock, at: i64) -> i64 {
    let offset = i64::from(clock.offset(at));
    local_days(at, clock.offset(at)) * DAY - offset
}

/// `at` as the store writes times, in UTC
/// (`2026-10-11T00:00:00.000Z`): what its spend is counted from.
pub fn stamp(at: i64) -> String {
    let (year, month, day) = civil(at.div_euclid(DAY));
    let second = at.rem_euclid(DAY);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.000Z",
        second / 3600,
        second % 3600 / 60,
        second % 60
    )
}

/// The weekday `at` falls on locally, Monday 0 to Sunday 6.
pub fn weekday(clock: &dyn Clock, at: i64) -> usize {
    // 1970-01-01 was a Thursday, 3 days after a Monday.
    (local_days(at, clock.offset(at)) + 3).rem_euclid(7) as usize
}

/// `at` as the host's local date and time, `YYYY-MM-DD HH:MM`: how an
/// agent reads and writes a time of its own schedules.
pub fn local(clock: &dyn Clock, at: i64) -> String {
    let local = at + i64::from(clock.offset(at));
    let (year, month, day) = civil(local.div_euclid(DAY));
    let second = local.rem_euclid(DAY);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        second / 3600,
        second % 3600 / 60
    )
}

/// `YYYY-MM-DD HH:MM`, the host's local time, in seconds since the
/// epoch.
pub fn parse_local(clock: &dyn Clock, text: &str) -> Option<i64> {
    let (date, time) = text.trim().split_once(' ')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    let wall = days * DAY + time_of_day(time)?;
    // The offset is the one at that time, not now's: across a change
    // of the clocks, the first guess is an hour out.
    let guess = wall - i64::from(clock.offset(wall));
    Some(wall - i64::from(clock.offset(guess)))
}

/// The days from 1970-01-01 to a calendar date, if it is one: the
/// inverse of [`civil`].
fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    let long = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return None,
    };
    if !(1..=long).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year.rem_euclid(400);
    let mp = i64::from((month + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

/// The calendar date `days` after 1970-01-01, as Howard Hinnant's
/// `civil_from_days` gives it.
fn civil(days: i64) -> (i64, u32, u32) {
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
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// The days in `month` of `year`, by the Gregorian leap rule.
    fn month_days(year: i64, month: u32) -> u32 {
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        match month {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    /// A `YYYY-MM-DD` day as its year, month and day.
    fn read_day(text: &str) -> (i64, u32, u32) {
        let mut parts = text.split('-');
        let mut part = || parts.next().unwrap().parse::<i64>().unwrap();
        let (year, month, day) = (part(), part(), part());
        (year, month as u32, day as u32)
    }

    /// The calendar day after `date`.
    fn next((year, month, day): (i64, u32, u32)) -> (i64, u32, u32) {
        if day < month_days(year, month) {
            (year, month, day + 1)
        } else if month < 12 {
            (year, month + 1, 1)
        } else {
            (year + 1, 1, 1)
        }
    }

    /// The calendar day before `date`.
    fn previous((year, month, day): (i64, u32, u32)) -> (i64, u32, u32) {
        if day > 1 {
            (year, month, day - 1)
        } else if month > 1 {
            (year, month - 1, month_days(year, month - 1))
        } else {
            (year - 1, 12, 31)
        }
    }

    /// Two 400-year eras either side of 1970, walked a day at a time
    /// from 1970-01-01, a Thursday: each day is the date the walk
    /// reached, reads back to its count, and has the weekday after the
    /// day before's; a day past its month's end is no date.
    #[test]
    fn the_calendar_is_walked_one_day_at_a_time() {
        const ERAS: i64 = 2 * 146_097;
        let utc = FakeClock::new(0, 0);
        assert_eq!(weekday(&utc, 0), 3);
        for (step, walk) in [
            (1, next as fn((i64, u32, u32)) -> (i64, u32, u32)),
            (-1, previous),
        ] {
            let mut date = (1970, 1, 1);
            let mut days: i64 = 0;
            while days.abs() <= ERAS {
                assert_eq!(civil(days), date, "{days}");
                let (year, month, day) = date;
                assert_eq!(days_from_civil(year, month, day), Some(days));
                if day == month_days(year, month) {
                    assert_eq!(days_from_civil(year, month, day + 1), None);
                }
                let at = days * DAY;
                assert_eq!(
                    weekday(&utc, at + step * DAY),
                    (weekday(&utc, at) as i64 + step).rem_euclid(7) as usize
                );
                days += step;
                date = walk(date);
            }
        }
        assert_eq!(days_from_civil(2026, 13, 1), None);
        assert_eq!(days_from_civil(2026, 0, 1), None);
    }

    /// For any time and any offset to 14 hours either way, in quarter
    /// hours: the local time reads back as the minute it names, however
    /// it is spaced; the local day begins at its midnight, which reads
    /// `00:00`, and the second before is the day before; and the store's
    /// UTC stamp is the UTC local time with its seconds.
    #[hegel::test(test_cases = 500)]
    fn local_times_read_back_and_days_begin_at_midnight(tc: TestCase) {
        let at = tc.draw(
            gs::integers::<i64>()
                .min_value(-4_000_000_000)
                .max_value(8_000_000_000),
        );
        let quarters = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![-56_i32, 0, 56]),
            gs::integers::<i32>().min_value(-56).max_value(56),
        ));
        let clock = FakeClock::new(at, quarters * 900);
        let minute = at - at.rem_euclid(60);
        let written = local(&clock, at);
        assert_eq!(parse_local(&clock, &written), Some(minute), "{written}");
        assert_eq!(parse_local(&clock, &format!("  {written} ")), Some(minute));
        let midnight = midnight(&clock, at);
        assert!(midnight <= at && at < midnight + DAY, "{midnight} {at}");
        assert_eq!(day(&clock, midnight), day(&clock, at));
        assert!(local(&clock, midnight).ends_with(" 00:00"));
        assert!(written.starts_with(&day(&clock, at)));
        assert_eq!(
            next(read_day(&day(&clock, midnight - 1))),
            read_day(&day(&clock, at))
        );
        let utc = FakeClock::new(at, 0);
        assert_eq!(
            stamp(at),
            format!(
                "{}:{:02}.000Z",
                local(&utc, at).replace(' ', "T"),
                at.rem_euclid(60)
            )
        );
    }

    /// What is not a local time, or not a date, reads as none.
    #[test]
    fn what_is_not_a_local_time_reads_as_none() {
        let clock = FakeClock::new(0, 3 * 3600);
        for text in [
            "2026-02-29 09:00",
            "2026-13-01 09:00",
            "2026-10-12",
            "2026-10-12 25:00",
            "2026-10-12 09:60",
            "2026-10-12-01 09:00",
        ] {
            assert_eq!(parse_local(&clock, text), None, "{text:?}");
        }
        assert!(parse_local(&clock, "2028-02-29 09:00").is_some());
    }

    /// The machine's offset is its time zone's at the time asked about:
    /// three hours east, UTC, five and a half hours west, and Central
    /// Europe in winter and in summer. Each zone is checked in a child
    /// process of this test binary that starts with `TZ` set, so no test
    /// changes the environment another one reads, under nextest or
    /// `cargo test` alike.
    #[test]
    fn the_system_clock_follows_the_time_zone() {
        const NAME: &str =
            "clock::tests::the_system_clock_follows_the_time_zone";
        const CHILD: &str = "TAU_SCHEDULE_OFFSETS";
        // 2026-01-15 and 2026-07-15, 12:00 UTC.
        const WINTER: i64 = 1_768_478_400;
        const SUMMER: i64 = 1_784_116_800;
        if let Ok(offsets) = std::env::var(CHILD) {
            for pair in offsets.split(',') {
                let (at, expected) = pair.split_once('=').unwrap();
                let at: i64 = at.parse().unwrap();
                assert_eq!(
                    SystemClock.offset(at).to_string(),
                    expected,
                    "{at}"
                );
            }
            return;
        }
        for (zone, offsets) in [
            ("<+03>-3", vec![(WINTER, 10_800), (SUMMER, 10_800)]),
            ("UTC0", vec![(WINTER, 0), (SUMMER, 0)]),
            ("<-0530>5:30", vec![(WINTER, -19_800)]),
            (
                "CET-1CEST,M3.5.0,M10.5.0/3",
                vec![(WINTER, 3_600), (SUMMER, 7_200)],
            ),
        ] {
            let offsets: Vec<String> = offsets
                .iter()
                .map(|(at, offset)| format!("{at}={offset}"))
                .collect();
            let out =
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args([NAME, "--exact", "--test-threads=1"])
                    .env("TZ", zone)
                    .env(CHILD, offsets.join(","))
                    .output()
                    .unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && stdout.contains("1 passed"),
                "TZ={zone}: {stdout}{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
