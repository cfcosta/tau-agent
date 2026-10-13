//! The message an agent's events reach it as (ADR 0037): tau's, not the
//! person's, one for every batch the coalescing window gathered.
//!
//! ```text
//! [events · 2 · 14:27:03–14:27:31]
//! - evening · schedule · evening:1791900000 · 14:27:03
//!   {"due":1791900000}
//! - triage · command · cmd:9f2c · 14:27:31
//!   {"args":"412","from":"tau-agent"}
//! ```
//!
//! Each event is its trigger's name, its kind, its id, when it came, and
//! its data as JSON on the line under it. [`Batch::message`] writes it;
//! [`Message::parse`] reads it back for the card the interface draws in
//! place of the text.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most of an event's data a message holds inline. The host keeps a
/// larger payload as a codemode artifact and puts its reference in the
/// line (ADR 0037); data it could not keep is cut here, and the line
/// says how much there was.
pub const INLINE_DATA: usize = 8 * 1024;

/// What fired an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Schedule,
    Command,
    Poll,
    Stream,
    Mcp,
    Message,
    /// A version of its `agent.luau` that did not load or whose tests
    /// failed: the agent hears so it can fix it.
    Version,
    /// The person's answer to one of its questions (`ask`).
    Answer,
    /// What it scheduled for itself from its conversation came due.
    Scheduled,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Schedule,
        Kind::Command,
        Kind::Poll,
        Kind::Stream,
        Kind::Mcp,
        Kind::Message,
        Kind::Version,
        Kind::Answer,
        Kind::Scheduled,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Schedule => "schedule",
            Kind::Command => "command",
            Kind::Poll => "poll",
            Kind::Stream => "stream",
            Kind::Mcp => "mcp",
            Kind::Message => "message",
            Kind::Version => "version",
            Kind::Answer => "answer",
            Kind::Scheduled => "scheduled",
        }
    }

    fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// One event, as the agent's queue keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// The trigger that fired it, by its name in `triggers`.
    pub trigger: String,
    pub kind: Kind,
    /// Unique per trigger: a schedule's due time, a poll's key, a
    /// command's own. A repeat after a restart has the same one.
    pub id: String,
    /// When it came, in seconds since the epoch.
    pub at: i64,
    pub data: Value,
}

/// The events one turn hears, in the order they came.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Batch {
    pub events: Vec<Event>,
}

impl Batch {
    /// The message the agent reads, its times at `utc_offset` seconds
    /// from UTC: the host's local time.
    pub fn message(&self, utc_offset: i32) -> String {
        let at = |seconds: i64| clock(seconds, utc_offset);
        let from = self.events.iter().map(|event| event.at).min();
        let to = self.events.iter().map(|event| event.at).max();
        let span = match (from, to) {
            (Some(from), Some(to)) if at(from) != at(to) => {
                format!("{}–{}", at(from), at(to))
            }
            (Some(from), _) => at(from),
            _ => String::new(),
        };
        let mut text = format!("[events · {} · {span}]", self.events.len());
        for event in &self.events {
            text.push_str(&format!(
                "\n- {} · {} · {} · {}\n  {}",
                field(&event.trigger),
                event.kind.as_str(),
                field(&event.id),
                at(event.at),
                inline(&event.data),
            ));
        }
        text
    }
}

/// An event message read back: what its card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The times it spans, as the header says them.
    pub span: String,
    pub events: Vec<Shown>,
}

/// One event of a [`Message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub trigger: String,
    pub kind: Kind,
    pub id: String,
    /// When it came, `HH:MM:SS`.
    pub time: String,
    /// Its data, as the message holds it: JSON, or JSON cut short.
    pub data: String,
}

impl Message {
    /// `text` as an event message, if it is one: tau writes them, so
    /// one the person typed that only looks like one still reads as
    /// one, and draws as a card.
    pub fn parse(text: &str) -> Option<Message> {
        let mut lines = text.lines();
        let header = lines
            .next()?
            .strip_prefix("[events · ")?
            .strip_suffix(']')?;
        let (count, span) = header.split_once(" · ")?;
        let count: usize = count.parse().ok()?;
        let mut events = Vec::new();
        while let Some(line) = lines.next() {
            let fields: Vec<&str> =
                line.strip_prefix("- ")?.split(" · ").collect();
            let [trigger, kind, id, time] = fields[..] else {
                return None;
            };
            let data = lines.next()?.strip_prefix("  ")?;
            events.push(Shown {
                trigger: trigger.to_owned(),
                kind: Kind::parse(kind)?,
                id: id.to_owned(),
                time: time.to_owned(),
                data: data.to_owned(),
            });
        }
        (events.len() == count).then(|| Message {
            span: span.to_owned(),
            events,
        })
    }
}

/// `seconds` since the epoch as `HH:MM:SS`, `utc_offset` seconds from
/// UTC.
pub fn clock(seconds: i64, utc_offset: i32) -> String {
    let of_day = (seconds + i64::from(utc_offset)).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// A name or an id as one field of an event's line: on one line, and
/// without the separator.
fn field(text: &str) -> String {
    text.replace(['\n', '\r'], " ").replace(" · ", " ")
}

/// `data` as compact JSON, cut at [`INLINE_DATA`] with a note of how
/// long it was.
fn inline(data: &Value) -> String {
    let json = data.to_string();
    if json.len() <= INLINE_DATA {
        return json;
    }
    let mut cut = INLINE_DATA;
    while !json.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{} … (cut: {} bytes in all)", &json[..cut], json.len())
}
