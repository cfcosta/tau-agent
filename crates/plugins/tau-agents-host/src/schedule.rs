//! When an agent's schedules come due (ADR 0037): every interval, or at
//! a time of the host's local day on some days of the week.
//!
//! A schedule keeps the next time it is due. Once that time passes, it
//! fires once, with that time as `due`, and moves past now: a schedule
//! that came due several times while tau was closed fires once.

use tau_agents::{TriggerKind, When};

use crate::clock::{self, Clock};
/// Durations and times of day, as tau-schedule reads them for chats too.
pub use crate::clock::{duration, time_of_day};

/// The shortest interval a schedule may have: an event costs a turn.
pub const SHORTEST: i64 = 60;

/// The longest an agent may schedule ahead, or between two times one
/// of its schedules fires.
pub const LONGEST: i64 = 366 * DAY;

const DAY: i64 = 86_400;

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// What a schedule is, read from its trigger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Schedule {
    /// Every so many seconds.
    Every(i64),
    /// At seconds into the local day, on these weekdays (Monday 0);
    /// every day when empty.
    At { seconds: i64, days: Vec<usize> },
}

impl Schedule {
    /// `kind` as a schedule, or why it is not one tau can keep.
    pub fn of(kind: &TriggerKind) -> Result<Self, String> {
        let TriggerKind::Schedule { every, at, days } = kind else {
            return Err("not a schedule".into());
        };
        Self::read(every.as_deref(), at.as_deref(), days)
    }

    /// What an agent scheduled for itself as a schedule, if it repeats;
    /// `None` for one that fires once.
    pub fn repeating(when: &When) -> Result<Option<Self>, String> {
        match when {
            When::Once { .. } => Ok(None),
            When::Every { every } => {
                Self::read(Some(every), None, &[]).map(Some)
            }
            When::Daily { at, days } => {
                Self::read(None, Some(at), days).map(Some)
            }
        }
    }

    fn read(
        every: Option<&str>,
        at: Option<&str>,
        days: &[String],
    ) -> Result<Self, String> {
        match (every, at) {
            (Some(every), None) => {
                let seconds = duration(every).ok_or_else(|| {
                    format!("`{every}` is not a duration, like \"15m\"")
                })?;
                if seconds < SHORTEST {
                    return Err(format!(
                        "`{every}` is shorter than a minute, the shortest a \
                         schedule may be"
                    ));
                }
                Ok(Self::Every(seconds))
            }
            (None, Some(at)) => {
                let seconds = time_of_day(at).ok_or_else(|| {
                    format!("`{at}` is not a time of day, like \"09:00\"")
                })?;
                let days = days
                    .iter()
                    .map(|day| {
                        WEEKDAYS
                            .iter()
                            .position(|known| known == day)
                            .ok_or_else(|| format!("`{day}` is not a day"))
                    })
                    .collect::<Result<_, _>>()?;
                Ok(Self::At { seconds, days })
            }
            _ => Err("a schedule is `every` or `at`".into()),
        }
    }

    /// The first time it is due after `after`.
    pub fn first_after(&self, after: i64, clock: &dyn Clock) -> i64 {
        match self {
            Self::Every(seconds) => after + seconds,
            Self::At { seconds, days } => {
                let midnight = clock::midnight(clock, after);
                (0..8)
                    .map(|day| midnight + day * DAY + seconds)
                    .find(|due| {
                        *due > after
                            && (days.is_empty()
                                || days.contains(&clock::weekday(clock, *due)))
                    })
                    .unwrap_or(after + DAY)
            }
        }
    }

    /// The time it is due next once it fired at `due` and it is `now`:
    /// past now, so the times it missed fire as one.
    pub fn after_firing(&self, due: i64, now: i64, clock: &dyn Clock) -> i64 {
        match self {
            Self::Every(seconds) => {
                let missed = (now - due).max(0) / seconds;
                due + (missed + 1) * seconds
            }
            Self::At { .. } => self.first_after(now.max(due), clock),
        }
    }
}

#[cfg(test)]
mod tests {
    use hegel::generators::{self as gs, Generator as _};

    use super::*;
    use crate::clock::FakeClock;

    /// A duration is a whole number above zero and a unit, `s`, `m`, `h`
    /// or `d`, maybe padded with spaces; anything else, or one too large to
    /// count in seconds, is none. A poll goes by one of at least a minute
    /// ([`crate::poll::every`]), and so does a schedule's `every`; each
    /// says why it cannot go by another.
    #[hegel::test(test_cases = 500)]
    fn a_duration_is_a_count_and_a_unit(tc: hegel::TestCase) {
        let count = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![-1i64, 0, 1, 59, 60, 61, i64::MAX]),
            gs::integers::<i64>().min_value(1).max_value(100_000),
        ));
        let unit = tc.draw(gs::sampled_from(vec!["s", "m", "h", "d", "w", ""]));
        let pad = tc.draw(gs::sampled_from(vec!["", " ", "  "]));
        let text = format!("{pad}{count}{unit}{pad}");
        let scale = match unit {
            "s" => Some(1i128),
            "m" => Some(60),
            "h" => Some(3_600),
            "d" => Some(86_400),
            _ => None,
        };
        let seconds = scale
            .filter(|_| count > 0)
            .map(|scale| i128::from(count) * scale)
            .and_then(|seconds| i64::try_from(seconds).ok());
        assert_eq!(duration(&text), seconds, "{text:?}");

        let every = crate::poll::every(&text);
        let schedule = Schedule::of(&TriggerKind::Schedule {
            every: Some(text.clone()),
            at: None,
            days: Vec::new(),
        });
        match seconds {
            Some(seconds) if seconds >= 60 => {
                assert_eq!(every, Ok(seconds));
                assert_eq!(schedule, Ok(Schedule::Every(seconds)));
            }
            Some(_) => {
                tc.event("under a minute");
                assert!(every.unwrap_err().contains("shorter than a minute"));
                assert!(
                    schedule.unwrap_err().contains("shorter than a minute")
                );
            }
            None => {
                assert!(every.unwrap_err().contains("not a duration"));
                assert!(schedule.unwrap_err().contains("not a duration"));
            }
        }
    }

    /// A time of day is `HH:MM`, hours under 24 and minutes under 60,
    /// with or without a leading zero; it is that many seconds into the
    /// day.
    #[hegel::test(test_cases = 500)]
    fn a_time_of_day_is_hours_and_minutes(tc: hegel::TestCase) {
        let hours = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![0i64, 23, 24]),
            gs::integers::<i64>().min_value(0).max_value(30),
        ));
        let minutes = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![0i64, 59, 60]),
            gs::integers::<i64>().min_value(0).max_value(70),
        ));
        let text = if tc.draw(gs::booleans()) {
            format!("{hours:02}:{minutes:02}")
        } else {
            format!("{hours}:{minutes}")
        };
        let expected = (hours < 24 && minutes < 60)
            .then_some(hours * 3_600 + minutes * 60);
        assert_eq!(time_of_day(&text), expected, "{text:?}");
        assert_eq!(time_of_day(&format!("{hours}")), None);
    }

    /// 2026-10-12T00:00:00Z, a Monday's midnight in UTC: weekdays are
    /// counted from it, apart from the clock's own reckoning.
    const MONDAY: i64 = 1_791_763_200;

    /// An interval's next time, once it fired at `due` and it is `now`, is
    /// the first of `due` and the intervals after it that is past now, and
    /// never `due` itself: the times it missed fire as one. Counted one
    /// interval at a time, not divided. Its first time is an interval on.
    #[hegel::test(test_cases = 500)]
    fn an_interval_moves_to_its_first_time_past_now(tc: hegel::TestCase) {
        let every = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![60i64, 61, 3_600, DAY]),
            gs::integers::<i64>().min_value(60).max_value(2 * DAY),
        ));
        let due =
            tc.draw(gs::integers::<i64>().min_value(0).max_value(1 << 40));
        // Early, on time, a moment late, exactly some intervals late, or
        // anywhere up to ten intervals late.
        let late = tc.draw(hegel::one_of!(
            gs::integers::<i64>().min_value(-every).max_value(1),
            gs::integers::<i64>()
                .min_value(0)
                .max_value(10)
                .map(move |n| n * every),
            gs::integers::<i64>().min_value(0).max_value(10 * every),
        ));
        let now = due + late;
        let clock = FakeClock::new(now, 0);
        let schedule = Schedule::Every(every);
        let mut next = due + every;
        while next <= now {
            next += every;
        }
        if late >= every {
            tc.event("missed some");
        }
        assert_eq!(schedule.after_firing(due, now, &clock), next);
        assert_eq!(schedule.first_after(now, &clock), now + every);
    }

    /// A time of day is due at that time of the host's local day, at any
    /// offset from UTC, on its days of the week (every day when none):
    /// its first time after a moment is past it, at the time, on a day it
    /// keeps, with no such time between, even from one of its own times.
    /// Once it fired, its next time is the first past both its due time
    /// and now.
    #[hegel::test(test_cases = 500)]
    fn a_time_of_day_comes_on_its_days_at_the_hosts_time(tc: hegel::TestCase) {
        let offset = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![-14 * 3600, -3600, 0, 3 * 3600, 14 * 3600]),
            gs::integers::<i32>()
                .min_value(-14 * 3600)
                .max_value(14 * 3600),
        ));
        let seconds = tc.draw(hegel::one_of!(
            gs::sampled_from(vec![0i64, 60, DAY - 60]),
            gs::integers::<i64>()
                .min_value(0)
                .max_value(DAY / 60 - 1)
                .map(|m| m * 60),
        ));
        let days: Vec<usize> =
            tc.draw(gs::subsequences((0..7).collect::<Vec<usize>>()));
        // It fired at one of its times: that time of some local day.
        let day = tc.draw(gs::integers::<i64>().min_value(-30).max_value(30));
        let due = MONDAY + day * DAY + seconds - i64::from(offset);
        // At its time or a moment either side, whole days on (at its
        // later times), or anywhere.
        let late = tc.draw(hegel::one_of!(
            gs::integers::<i64>().min_value(-2).max_value(2),
            gs::integers::<i64>()
                .min_value(0)
                .max_value(8)
                .map(|n| n * DAY),
            gs::integers::<i64>().min_value(-DAY).max_value(9 * DAY),
        ));
        let now = due + late;
        let clock = FakeClock::new(now, offset);
        let schedule = Schedule::At {
            seconds,
            days: days.clone(),
        };
        let local = |at: i64| at + i64::from(offset);
        let weekday =
            |at: i64| (local(at) - MONDAY).div_euclid(DAY).rem_euclid(7);
        let keeps = |at: i64| {
            local(at).rem_euclid(DAY) == seconds
                && (days.is_empty() || days.contains(&(weekday(at) as usize)))
        };
        let first_after = |after: i64, got: i64| {
            assert!(got > after, "{got} is not past {after}");
            assert!(keeps(got), "{got} is not on the schedule {days:?}");
            // The schedule's earlier times are a whole number of days
            // before: none of them is past `after` and kept.
            for back in 1..=7 {
                let earlier = got - back * DAY;
                assert!(
                    earlier <= after || !keeps(earlier),
                    "{earlier} came first"
                );
            }
        };
        first_after(now, schedule.first_after(now, &clock));
        first_after(now.max(due), schedule.after_firing(due, now, &clock));
        if !days.is_empty() && days.len() < 7 {
            tc.event("some days");
        }
    }
}
