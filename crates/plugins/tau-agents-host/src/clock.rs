//! The host's clock: what time it is, and the host's local days, which
//! an agent's schedules and its daily cap go by (ADR 0037).

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

/// The weekday `at` falls on locally, Monday 0 to Sunday 6.
pub fn weekday(clock: &dyn Clock, at: i64) -> usize {
    // 1970-01-01 was a Thursday, 3 days after a Monday.
    (local_days(at, clock.offset(at)) + 3).rem_euclid(7) as usize
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
    use super::*;

    #[test]
    fn days_are_the_hosts_local_ones() {
        // 2026-10-11T23:30:00Z, a Sunday.
        let at = 1_791_761_400;
        let utc = FakeClock::new(at, 0);
        assert_eq!(day(&utc, at), "2026-10-11");
        assert_eq!(weekday(&utc, at), 6);
        assert_eq!(midnight(&utc, at), at - 23 * 3600 - 1800);
        // Three hours east, it is Monday already.
        let east = FakeClock::new(at, 3 * 3600);
        assert_eq!(day(&east, at), "2026-10-12");
        assert_eq!(weekday(&east, at), 0);
        assert_eq!(midnight(&east, at), at - 2 * 3600 - 1800);
        // Three hours west, still Sunday.
        let west = FakeClock::new(at, -3 * 3600);
        assert_eq!(day(&west, at), "2026-10-11");
        assert_eq!(midnight(&west, at), at - 20 * 3600 - 1800);
    }

    #[test]
    fn the_calendar_holds_across_eras() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(-1), (1969, 12, 31));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(20_737), (2026, 10, 11));
    }

    #[test]
    fn the_system_clock_has_an_offset() {
        let clock = SystemClock;
        let offset = clock.offset(clock.now());
        assert!(offset.abs() <= 14 * 3600, "{offset}");
    }
}
