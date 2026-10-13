//! What an agent schedules for itself from its conversation (ADR 0037):
//! once, or again and again, kept in tau's state beside its queue rather
//! than in its `agent.luau`. Each fires an event with the note the agent
//! left itself, the person sees them on its page and may cancel them,
//! and one that came due while tau was closed fires once, as a declared
//! schedule does.

use serde::Deserialize;
use serde_json::json;
use tau_agents::{
    Scheduled,
    When,
    event::{Event, Kind},
};

use super::{Agents, talk::MAX_TEXT};
use crate::{
    clock,
    schedule::{self, LONGEST, SHORTEST, Schedule},
};

/// The most an agent may have scheduled at once: one that schedules in
/// a loop stops here, not at its cap.
pub const MAX_SCHEDULED: usize = 100;

/// What an agent asks to schedule: a name, a note, and exactly one of
/// `in` or `on` (once) or `every` or `at` (again and again).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub name: String,
    pub note: String,
    /// Once, after this long: `"2h"`.
    #[serde(default, rename = "in")]
    pub after: Option<String>,
    /// Once, at this local date and time: `"2026-10-13 09:00"`.
    #[serde(default)]
    pub on: Option<String>,
    /// Every interval: `"15m"`.
    #[serde(default)]
    pub every: Option<String>,
    /// At a local time each day: `"09:00"`.
    #[serde(default)]
    pub at: Option<String>,
    /// With `at`, the days: `Mon` to `Sun`; none is every day.
    #[serde(default)]
    pub days: Vec<String>,
}

/// What [`Agents::schedule`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    pub scheduled: Scheduled,
    /// It took the place of one of the same name.
    pub replaced: bool,
}

/// Whether `name` can name a schedule: lower-case letters, digits, `-`
/// and `_`, starting with a letter, at most 40.
fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
        })
}

impl Agents {
    /// Schedules `plan` for `agent`, in place of one of the same name.
    /// Why not, in words for the agent.
    pub async fn schedule(
        &self,
        agent: &str,
        plan: Plan,
    ) -> Result<Made, String> {
        let now = self.0.clock.now();
        let name = plan.name.trim().to_owned();
        if !valid_name(&name) {
            return Err(format!(
                "`{name}` cannot name a schedule: lower-case letters, digits, \
                 `-` and `_`, starting with a letter, at most 40"
            ));
        }
        let declared = self.0.registry.live(agent).await.is_some_and(|live| {
            live.declared
                .triggers
                .iter()
                .any(|known| known.name == name)
        });
        if declared || name == "ask" {
            return Err(format!(
                "`{name}` is a trigger of yours already: name it otherwise"
            ));
        }
        let note = plan.note.trim().to_owned();
        if note.is_empty() {
            return Err("Leave yourself a note: what to do then".into());
        }
        if note.chars().count() > MAX_TEXT {
            return Err(format!("Keep the note under {MAX_TEXT} characters"));
        }
        let clock = self.0.clock.as_ref();
        let (when, next) = match (plan.after, plan.on, plan.every, plan.at) {
            (Some(after), None, None, None) => {
                let seconds = schedule::duration(&after).ok_or_else(|| {
                    format!("`{after}` is not a duration, like \"2h\"")
                })?;
                let at = now + seconds;
                (When::Once { at }, at)
            }
            (None, Some(on), None, None) => {
                let at = clock::parse_local(clock, &on).ok_or_else(|| {
                    format!(
                        "`{on}` is not a local date and time, like \"{}\"",
                        clock::local(clock, now + 86_400)
                    )
                })?;
                (When::Once { at }, at)
            }
            (None, None, Some(every), None) => {
                let when = When::Every { every };
                let first = Schedule::repeating(&when)?
                    .expect("repeats")
                    .first_after(now, clock);
                (when, first)
            }
            (None, None, None, Some(at)) => {
                let when = When::Daily {
                    at,
                    days: plan.days,
                };
                let first = Schedule::repeating(&when)?
                    .expect("repeats")
                    .first_after(now, clock);
                (when, first)
            }
            _ => {
                return Err("Say when with one of `in` or `on`, for once, or \
                            `every` or `at`, for again and again"
                    .into());
            }
        };
        if let When::Once { .. } = when
            && next < now + SHORTEST
        {
            return Err(format!(
                "That is {}: a minute from now is the soonest",
                clock::local(clock, next)
            ));
        }
        if next > now + LONGEST
            || matches!(&when, When::Every { every }
                if schedule::duration(every).is_some_and(|every| every > LONGEST))
        {
            return Err("A year ahead is the furthest".into());
        }
        let scheduled = Scheduled {
            name: name.clone(),
            note,
            when,
            next,
            last: None,
        };
        let made = self
            .with_kept(agent, |kept| {
                let replaced = kept.scheduled.contains_key(&name);
                if !replaced && kept.scheduled.len() >= MAX_SCHEDULED {
                    return Err(format!(
                        "You have {MAX_SCHEDULED} scheduled, the most you may: \
                         cancel some first"
                    ));
                }
                kept.scheduled.insert(name, scheduled.clone());
                Ok(Made {
                    scheduled,
                    replaced,
                })
            })
            .await?;
        self.0.env.refresh();
        Ok(made)
    }

    /// Cancels what `agent` scheduled as `name`: whether there was one.
    pub async fn unschedule(&self, agent: &str, name: &str) -> bool {
        let gone = self
            .with_kept(agent, |kept| kept.scheduled.remove(name.trim()))
            .await
            .is_some();
        if gone {
            self.0.env.refresh();
        }
        gone
    }

    /// What `agent` has scheduled, soonest first.
    pub async fn scheduled(&self, agent: &str) -> Vec<Scheduled> {
        let mut scheduled: Vec<Scheduled> = self
            .with_kept(agent, |kept| kept.scheduled.values().cloned().collect())
            .await;
        scheduled.sort_by(|a, b| (a.next, &a.name).cmp(&(b.next, &b.name)));
        scheduled
    }

    /// What time it is, by the agents' clock.
    pub fn now(&self) -> i64 {
        self.0.clock.now()
    }

    /// `at` as the host's local date and time, as the agent reads and
    /// writes its schedules' times.
    pub fn local_time(&self, at: i64) -> String {
        clock::local(self.0.clock.as_ref(), at)
    }

    /// Queues what `agent` scheduled that came due: one that fires once
    /// goes, one that repeats moves past now. Whether anything did.
    pub(super) async fn fire_scheduled(&self, agent: &str, now: i64) -> bool {
        let clock = self.0.clock.clone();
        self.with_kept(agent, |kept| {
            let due: Vec<String> = kept
                .scheduled
                .values()
                .filter(|scheduled| scheduled.next <= now)
                .map(|scheduled| scheduled.name.clone())
                .collect();
            for name in &due {
                let scheduled = kept.scheduled.get(name).expect("just seen");
                let repeats = Schedule::repeating(&scheduled.when);
                let event = Event {
                    trigger: name.clone(),
                    kind: Kind::Scheduled,
                    id: format!("{name}:{}", scheduled.next),
                    at: now,
                    data: json!({
                        "note": scheduled.note,
                        "due": scheduled.next,
                        "repeats": matches!(repeats, Ok(Some(_))),
                    }),
                };
                let known = kept.queue.iter().any(|queued| {
                    queued.trigger == event.trigger && queued.id == event.id
                });
                if !known {
                    kept.fired.insert(name.clone(), now);
                    kept.queue.push(event);
                }
                match repeats {
                    Ok(Some(schedule)) => {
                        let scheduled =
                            kept.scheduled.get_mut(name).expect("just seen");
                        scheduled.next = schedule.after_firing(
                            scheduled.next,
                            now,
                            clock.as_ref(),
                        );
                        scheduled.last = Some(now);
                    }
                    _ => {
                        kept.scheduled.remove(name);
                    }
                }
            }
            !due.is_empty()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_short_and_plain() {
        assert!(valid_name("check-pr-412"));
        assert!(valid_name("morning_digest"));
        assert!(!valid_name(""));
        assert!(!valid_name("412"));
        assert!(!valid_name("Check"));
        assert!(!valid_name("check pr"));
        assert!(!valid_name(&"a".repeat(41)));
    }
}
