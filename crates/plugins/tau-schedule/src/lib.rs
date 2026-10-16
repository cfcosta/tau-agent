//! tau-schedule: a chat wakes itself later
//! ([ADR 0039](../../../docs/decisions/0039-chats-wake-themselves-later.md)).
//!
//! A chat's `schedule` tool leaves it a note for a time to come; when
//! that time comes, the chat takes a turn of tau's on the note. Once
//! only: repeating is for agents (ADR 0037), whose own `schedule` takes
//! this one's place in their runs.
//!
//! - [`Wake`], [`Overview`]: what chats have waiting, for every
//!   interface.
//! - [`Act`]: what the interface asks the host.
//! - [`message`], [`Woken::parse`]: the message a chat wakes on.
//! - `clock` (with `host`): the host's time and local days, which
//!   agents' schedules go by too.
//! - `host` (with `host`): what chats scheduled, kept on the host, and
//!   the turns that wake them.

#[cfg(feature = "host")]
pub mod clock;
#[cfg(feature = "host")]
pub mod host;
pub mod ui;

#[cfg(feature = "host")]
pub use host::{Made, Plan, ScheduleHost, Schedules};
use serde::{Deserialize, Serialize};
pub use ui::ScheduleUi;

/// The plugin's name, for both its halves.
pub const NAME: &str = "tau-schedule";

/// The most a chat may have waiting: one that schedules in a loop stops
/// here.
pub const MAX_WAITING: usize = 100;

/// A wake-up a chat scheduled for itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wake {
    /// The chat it wakes.
    pub run: String,
    /// Its own among the chat's: what cancelling it names.
    pub name: String,
    /// What the chat left itself for then.
    pub note: String,
    /// When it comes due, in seconds since the epoch.
    pub at: i64,
    /// When the chat scheduled it.
    pub made: i64,
}

/// What every chat has waiting, soonest first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overview {
    pub wakes: Vec<Wake>,
}

impl Overview {
    /// What the chat `run` has waiting, soonest first.
    pub fn of_run<'a>(
        &'a self,
        run: &'a str,
    ) -> impl Iterator<Item = &'a Wake> {
        self.wakes.iter().filter(move |wake| wake.run == run)
    }
}

/// What the interface asks the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "act", rename_all = "snake_case")]
pub enum Act {
    /// The person cancelled what the chat `run` scheduled as `name`.
    Cancel { run: String, name: String },
}

/// The message a chat wakes on: one block per wake-up that came due,
/// its name and when, in the host's local time, then its note.
///
/// ```text
/// [scheduled · recheck · 2026-10-13 09:30]
/// Did #412 merge? If not, look at what its review asks.
/// ```
pub fn message(woken: &[Woken]) -> String {
    woken
        .iter()
        .map(|woken| {
            format!(
                "[scheduled · {} · {}]\n{}",
                woken.name, woken.due, woken.note
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// One wake-up in the message a chat wakes on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Woken {
    pub name: String,
    /// When it came due, `YYYY-MM-DD HH:MM` in the host's local time.
    pub due: String,
    pub note: String,
}

impl Woken {
    /// `text` as the wake-ups of a message tau wrote, if it is one: the
    /// card the interface draws in its place.
    pub fn parse(text: &str) -> Option<Vec<Woken>> {
        let mut woken: Vec<Woken> = Vec::new();
        for line in text.lines() {
            let head = line
                .strip_prefix("[scheduled · ")
                .and_then(|rest| rest.strip_suffix(']'))
                .and_then(|rest| rest.split_once(" · "));
            match (head, woken.last_mut()) {
                (Some((name, due)), _) => woken.push(Woken {
                    name: name.to_owned(),
                    due: due.to_owned(),
                    note: String::new(),
                }),
                (None, Some(last)) => {
                    if !last.note.is_empty() {
                        last.note.push('\n');
                    }
                    last.note.push_str(line);
                }
                (None, None) => return None,
            }
        }
        for woken in &mut woken {
            woken.note = woken.note.trim_end().to_owned();
        }
        (!woken.is_empty()).then_some(woken)
    }
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// A note as a chat writes one: lines of text, blank ones only
    /// between them, none that looks like a wake-up's header.
    fn note(tc: &TestCase) -> String {
        let line = || {
            gs::from_regex("[A-Za-z0-9#?.,:'() -]{0,24}[A-Za-z0-9?.]")
                .fullmatch(true)
        };
        let mut lines: Vec<String> = vec![tc.draw(line())];
        for more in tc.draw(gs::vecs(gs::optional(line())).max_size(3)) {
            lines.push(more.unwrap_or_default());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    fn woken(tc: &TestCase) -> Woken {
        Woken {
            name: tc
                .draw(gs::from_regex("[a-z][a-z0-9_-]{0,12}").fullmatch(true)),
            due: format!(
                "2026-{:02}-{:02} {:02}:{:02}",
                tc.draw(gs::integers::<u8>().min_value(1).max_value(12)),
                tc.draw(gs::integers::<u8>().min_value(1).max_value(28)),
                tc.draw(gs::integers::<u8>().max_value(23)),
                tc.draw(gs::integers::<u8>().max_value(59)),
            ),
            note: note(tc),
        }
    }

    /// The wake-ups a message names read back from it as they were, in
    /// order; text that does not start with a wake-up's header is no
    /// message.
    #[hegel::test(test_cases = 300)]
    fn a_message_reads_back_as_written(tc: TestCase) {
        let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
        let all: Vec<Woken> = (0..count).map(|_| woken(&tc)).collect();
        let text = message(&all);
        assert!(text.starts_with(&format!("[scheduled · {} · ", all[0].name)));
        assert_eq!(Woken::parse(&text), Some(all), "{text:?}");
        let before = tc.draw(gs::sampled_from(vec!["", "Did it merge?", " "]));
        assert_eq!(Woken::parse(&format!("{before}\n{text}")), None);
        assert_eq!(Woken::parse(before), None);
    }
}
