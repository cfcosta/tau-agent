//! When an agent's schedules come due (ADR 0037): every interval, or at
//! a time of the host's local day on some days of the week.
//!
//! A schedule keeps the next time it is due. Once that time passes, it
//! fires once, with that time as `due`, and moves past now: a schedule
//! that came due several times while tau was closed fires once.

use tau_agents::TriggerKind;

use crate::clock::{self, Clock};

/// The shortest interval a schedule may have: an event costs a turn.
pub const SHORTEST: i64 = 60;

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

#[cfg(test)]
mod tests {
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

    #[test]
    fn durations_and_times_read_as_written() {
        assert_eq!(duration("15m"), Some(900));
        assert_eq!(duration("2h"), Some(7200));
        assert_eq!(duration("1d"), Some(86_400));
        assert_eq!(duration("30s"), Some(30));
        assert_eq!(duration("0m"), None);
        assert_eq!(duration("m"), None);
        assert_eq!(duration("5w"), None);
        assert_eq!(time_of_day("09:30"), Some(34_200));
        assert_eq!(time_of_day("24:00"), None);
        assert_eq!(time_of_day("9"), None);
        let short = Schedule::of(&TriggerKind::Schedule {
            every: Some("30s".into()),
            at: None,
            days: Vec::new(),
        });
        assert!(short.unwrap_err().contains("shorter than a minute"));
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
