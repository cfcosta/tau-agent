//! How every interface words an agent: its state, what wakes it, and
//! when, said relative to now so no window needs the host's time zone.

use crate::{Agent, Status, Trigger, TriggerKind, valid_name};

/// Now, in seconds since the epoch.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64)
}

/// `at` said from `now`: `in 6m`, `4m ago`, `now`.
pub fn relative(at: i64, now: i64) -> String {
    let gap = at - now;
    let span = |seconds: i64| match seconds {
        ..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86_400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86_400),
    };
    match gap {
        -59..=59 => "now".to_owned(),
        60.. => format!("in {}", span(gap)),
        _ => format!("{} ago", span(-gap)),
    }
}

impl Status {
    /// The state in a word or two, as its pill says it.
    pub fn label(&self) -> &'static str {
        match self {
            Status::Waiting => "waiting",
            Status::Working => "working",
            Status::Paused => "paused",
            Status::AtCap => "at its cap",
            Status::Broken { .. } => "broken",
        }
    }
}

impl Agent {
    /// Whether it waits on the person: it spent its day, nothing loads
    /// to wake it, or it left them a note or a question.
    pub fn needs_you(&self) -> bool {
        self.waits_on_you() > 0
    }

    /// How many things of its wait on the person: its notes and
    /// questions, and its state when it spent its day or nothing loads.
    pub fn waits_on_you(&self) -> usize {
        let state =
            matches!(self.status, Status::AtCap | Status::Broken { .. });
        self.notices.len() + usize::from(state)
    }

    /// When the first of its triggers fires next, if any says.
    pub fn next(&self) -> Option<i64> {
        self.triggers
            .iter()
            .filter_map(|trigger| trigger.next)
            .min()
    }

    /// The share of today's cap it spent, from 0 to 1.
    pub fn spent_share(&self) -> f32 {
        if self.cap_today <= 0.0 {
            return 1.0;
        }
        (self.spend_today / self.cap_today).clamp(0.0, 1.0) as f32
    }

    /// Its command triggers: `/command`, with the hint after it.
    pub fn commands(&self) -> impl Iterator<Item = (&str, Option<&str>)> {
        self.triggers
            .iter()
            .filter_map(|trigger| match &trigger.kind {
                TriggerKind::Command { command, hint } => {
                    Some((command.as_str(), hint.as_deref()))
                }
                _ => None,
            })
    }
}

impl Trigger {
    /// What wakes it, in a few words: `every 15m`, `09:00 Mon Fri`,
    /// `/triage <number>`.
    pub fn label(&self) -> String {
        match &self.kind {
            TriggerKind::Schedule { every, at, days } => {
                let when = match (every, at) {
                    (Some(every), _) => format!("every {every}"),
                    (None, Some(at)) => at.clone(),
                    (None, None) => "on a schedule".to_owned(),
                };
                if days.is_empty() {
                    when
                } else {
                    format!("{when} {}", days.join(" "))
                }
            }
            TriggerKind::Command { command, hint } => match hint {
                Some(hint) => format!("/{command} {hint}"),
                None => format!("/{command}"),
            },
            TriggerKind::Poll { source, every, .. } => {
                format!("{source} every {every}")
            }
            TriggerKind::Stream { url, .. } => url.clone(),
            TriggerKind::Mcp { server, uri } => format!("{server} {uri}"),
            TriggerKind::Message => "messages from agents".to_owned(),
        }
    }

    /// Its kind, as a tag says it.
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            TriggerKind::Schedule { .. } => "schedule",
            TriggerKind::Command { .. } => "command",
            TriggerKind::Poll { .. } => "poll",
            TriggerKind::Stream { .. } => "stream",
            TriggerKind::Mcp { .. } => "mcp",
            TriggerKind::Message => "message",
        }
    }
}

/// A name for an agent that does what `purpose` says: its first words
/// that name something, joined by dashes, or `agent`.
pub fn suggest_name(purpose: &str) -> String {
    const SKIP: [&str; 16] = [
        "a", "an", "the", "and", "or", "to", "of", "for", "in", "on", "my",
        "me", "it", "every", "each", "when",
    ];
    let words: Vec<String> = purpose
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| !word.is_empty() && !SKIP.contains(&word.as_str()))
        .take(3)
        .collect();
    let mut name = words.join("-");
    name.truncate(40);
    let name = name.trim_end_matches('-').to_owned();
    if valid_name(&name) {
        name
    } else {
        "agent".to_owned()
    }
}
