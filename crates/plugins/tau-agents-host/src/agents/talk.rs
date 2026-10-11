//! What an agent says to the person and to other agents (ADR 0037): its
//! notes and questions, the person's answers, which it hears as events,
//! and its messages to other agents.

use std::{path::Path, sync::atomic::Ordering};

use serde_json::{Value, json};
use tau_agents::{
    Last,
    Notice,
    TriggerKind,
    event::{Event, Kind, Message},
};

use super::Agents;

/// The most notes an agent keeps for the person: an older one goes when
/// a new one comes. Questions stay until answered.
const NOTES: usize = 20;

/// The longest note, question or choice, in characters: the person
/// reads them.
pub const MAX_TEXT: usize = 4000;

/// The longest message to another agent, in characters. One over its
/// message's inline data reaches it as an artifact (ADR 0037).
pub const MAX_MESSAGE: usize = 1_000_000;

/// How far a message may go, agent to agent, through turns that only
/// messages woke (ADR 0037's guard against ping-pong).
pub const MAX_HOPS: u64 = 8;

impl Agents {
    /// The agent whose repository is checked out at `checkout`, if it is
    /// one's.
    pub fn agent_at(&self, checkout: &Path) -> Option<String> {
        if checkout.parent() != Some(self.0.root.as_path()) {
            return None;
        }
        Some(checkout.file_name()?.to_str()?.to_owned())
    }

    /// Leaves the person `agent`'s note `text`. Its id.
    pub async fn notify(
        &self,
        agent: &str,
        text: &str,
    ) -> anyhow::Result<String> {
        self.leave(agent, text, None).await
    }

    /// Leaves the person `agent`'s question, with the answers it offers
    /// (none: any). Its id; the answer comes as an event.
    pub async fn ask(
        &self,
        agent: &str,
        question: &str,
        choices: Vec<String>,
    ) -> anyhow::Result<String> {
        for choice in &choices {
            anyhow::ensure!(!choice.trim().is_empty(), "A choice is empty");
            anyhow::ensure!(
                choice.chars().count() <= MAX_TEXT,
                "A choice is longer than {MAX_TEXT} characters"
            );
        }
        self.leave(agent, question, Some(choices)).await
    }

    /// What `agent` left that the person has not dealt with yet: its
    /// questions waiting for an answer, and its notes not dismissed,
    /// oldest first.
    pub async fn waiting(&self, agent: &str) -> Vec<Notice> {
        self.with_kept(agent, |kept| kept.notices.clone()).await
    }

    /// The id of `agent`'s question `question`, if the same one waits
    /// for an answer already.
    pub async fn asked(&self, agent: &str, question: &str) -> Option<String> {
        let question = question.trim();
        self.waiting(agent)
            .await
            .into_iter()
            .find(|notice| notice.asks() && notice.text == question)
            .map(|notice| notice.id)
    }

    async fn leave(
        &self,
        agent: &str,
        text: &str,
        choices: Option<Vec<String>>,
    ) -> anyhow::Result<String> {
        let text = text.trim();
        anyhow::ensure!(!text.is_empty(), "Say something");
        anyhow::ensure!(
            text.chars().count() <= MAX_TEXT,
            "Keep it under {MAX_TEXT} characters"
        );
        let at = self.0.clock.now();
        let id = self
            .with_kept(agent, |kept| {
                kept.noticed += 1;
                let id = format!("n{}", kept.noticed);
                if choices.is_none() {
                    kept.last = Some(Last {
                        at,
                        line: format!("said: {}", first_line(text)),
                    });
                }
                kept.notices.push(Notice {
                    id: id.clone(),
                    at,
                    text: text.to_owned(),
                    choices,
                });
                let notes =
                    kept.notices.iter().filter(|notice| !notice.asks()).count();
                if notes > NOTES
                    && let Some(oldest) =
                        kept.notices.iter().position(|notice| !notice.asks())
                {
                    kept.notices.remove(oldest);
                }
                id
            })
            .await;
        self.0.env.refresh();
        Ok(id)
    }

    /// The person answered `agent`'s question `notice`: it goes, and the
    /// agent hears the answer as an event.
    pub(super) async fn answer(
        &self,
        agent: &str,
        notice: &str,
        answer: &str,
    ) -> anyhow::Result<()> {
        let answer = answer.trim();
        anyhow::ensure!(!answer.is_empty(), "Write an answer");
        let now = self.0.clock.now();
        let asked = self
            .with_kept(agent, |kept| {
                let at = kept
                    .notices
                    .iter()
                    .position(|left| left.id == notice && left.asks())?;
                Some(kept.notices.remove(at))
            })
            .await
            .ok_or_else(|| {
                anyhow::anyhow!("{agent} has no question {notice} waiting")
            })?;
        self.enqueue(
            agent,
            Event {
                trigger: "ask".into(),
                kind: Kind::Answer,
                id: asked.id,
                at: now,
                data: json!({ "question": asked.text, "answer": answer }),
            },
        )
        .await;
        Ok(())
    }

    /// The person read `agent`'s note `notice`: it goes. A question goes
    /// only with an answer.
    pub(super) async fn dismiss(
        &self,
        agent: &str,
        notice: &str,
    ) -> anyhow::Result<()> {
        let gone = self
            .with_kept(agent, |kept| {
                let before = kept.notices.len();
                kept.notices.retain(|left| left.id != notice || left.asks());
                kept.notices.len() < before
            })
            .await;
        anyhow::ensure!(gone, "{agent} has no note {notice}");
        Ok(())
    }

    /// Puts `from`'s message `text` in `to`'s queue, `hops` from the
    /// first message of its chain ([`hops_of`]). It is refused, with
    /// why, past [`MAX_HOPS`], to an agent that hears no messages, and
    /// to the sender itself.
    pub async fn send(
        &self,
        from: &str,
        to: &str,
        text: &str,
        hops: u64,
    ) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("Say something".into());
        }
        if text.chars().count() > MAX_MESSAGE {
            return Err(format!("Keep it under {MAX_MESSAGE} characters"));
        }
        if to == from {
            return Err("That is you: keep what you would tell yourself \
                        in your notes or your memory"
                .into());
        }
        if hops > MAX_HOPS {
            return Err(format!(
                "Not sent: it would be the {hops}th message in a row \
                 between agents with nothing else waking them, and tau \
                 stops at {MAX_HOPS}. Do what is left yourself, or \
                 notify the person."
            ));
        }
        let live = self
            .0
            .registry
            .live(to)
            .await
            .ok_or_else(|| format!("No agent {to} is live"))?;
        let trigger = live
            .declared
            .triggers
            .iter()
            .find(|trigger| trigger.kind == TriggerKind::Message)
            .ok_or_else(|| {
                format!(
                    "{to} hears no messages: its agent.luau declares no \
                     `message = true` trigger"
                )
            })?;
        let now = self.0.clock.now();
        let id = format!(
            "msg:{from}:{now:x}{:02x}",
            self.0.commands.fetch_add(1, Ordering::Relaxed) % 256
        );
        self.enqueue(
            to,
            Event {
                trigger: trigger.name.clone(),
                kind: Kind::Message,
                id,
                at: now,
                data: json!({ "from": from, "text": text, "hops": hops }),
            },
        )
        .await;
        self.0.env.refresh();
        Ok(())
    }
}

/// How many hops a message goes when sent in a turn that heard `input`:
/// one more than the furthest message that woke the turn, when only
/// messages did; one when anything else did (the person, a schedule, a
/// poll), which starts a chain afresh.
pub fn hops_of(input: &str) -> u64 {
    let Some(message) = Message::parse(input) else {
        return 1;
    };
    if message.events.is_empty()
        || message
            .events
            .iter()
            .any(|event| event.kind != Kind::Message)
    {
        return 1;
    }
    let furthest = message
        .events
        .iter()
        .map(|event| {
            // A message whose data was cut, or does not say, counts as
            // far as a message may go: unread, it is not trusted short.
            serde_json::from_str::<Value>(&event.data)
                .ok()
                .and_then(|data| data.get("hops")?.as_u64())
                .unwrap_or(MAX_HOPS)
        })
        .max()
        .unwrap_or(0);
    furthest + 1
}

/// `text`'s first line, cut to a sidebar row's length.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default();
    match line.char_indices().nth(80) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line.to_owned(),
    }
}
