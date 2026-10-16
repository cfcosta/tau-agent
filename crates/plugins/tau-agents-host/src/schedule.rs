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
    use hegel::generators as gs;

    use super::*;
    use crate::clock::FakeClock;

    fn at(at: &str, days: &[&str]) -> Schedule {
        Schedule::of(&TriggerKind::Schedule {
            every: None,
            at: Some(at.into()),
            days: days.iter().map(|day| (*day).to_owned()).collect(),
        })
        .unwrap()
    }

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

    #[test]
    fn an_interval_skips_what_it_missed() {
        let clock = FakeClock::new(0, 0);
        let hourly = Schedule::Every(3600);
        assert_eq!(hourly.first_after(1_000, &clock), 4_600);
        // Due at 4 600, fired at 4 700: next at 8 200.
        assert_eq!(hourly.after_firing(4_600, 4_700, &clock), 8_200);
        // Five hours late: once, then the next hour from where it was.
        assert_eq!(
            hourly.after_firing(4_600, 4_600 + 5 * 3600 + 10, &clock),
            4_600 + 6 * 3600
        );
    }

    #[test]
    fn a_time_of_day_is_the_hosts_on_its_days() {
        // 2026-10-11T23:30:00Z, a Sunday; three hours east, it is Monday
        // 02:30.
        let now = 1_791_761_400;
        let east = FakeClock::new(now, 3 * 3600);
        let monday = clock::midnight(&east, now);
        let nine = at("09:00", &[]);
        assert_eq!(nine.first_after(now, &east), monday + 9 * 3600);
        // Weekdays only from Tuesday on: Tuesday 09:00.
        let tuesday = at("09:00", &["Tue", "Wed"]);
        assert_eq!(tuesday.first_after(now, &east), monday + DAY + 9 * 3600);
        // Once it fired, the next is a day on.
        let due = monday + 9 * 3600;
        assert_eq!(nine.after_firing(due, due + 5, &east), due + DAY);
        // Fired three days late: the next after now.
        assert_eq!(
            nine.after_firing(due, due + 3 * DAY + 3600, &east),
            due + 4 * DAY
        );
    }
}
