//! Replays stored chats through the effort policy, to see what it would
//! pick where the stored runs picked something else
//! (`docs/reference/plugins.md`, "Replaying stored runs").
//!
//! The replay walks a run's timeline request by request. A user message
//! that follows a final answer starts a message, which Jev scores as the
//! plugin's `start` would; a later request is scored again only when
//! the simulated lease has ended. It reports decisions, not savings:
//! what a request would have cost at another effort is unknown, since
//! the reasoning tokens it would have spent were never spent.

use serde_json::Value;
use tau_ai::{
    message::{InputBlock, Message, UserContent},
    responses::request::ReasoningEffort,
};

use crate::{
    Choice,
    Lease,
    Picker,
    context_record,
    lease_ended,
    message_state,
    step_state,
};

/// One entry of a stored run's timeline, as the replay reads it.
#[derive(Debug, Clone)]
pub enum Entry {
    Message(Message),
    /// A record of this plugin's.
    Record(Value),
}

/// What the policy decided before one stored request.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The request's place in the run, from 1.
    pub request: usize,
    /// `user_turn` or `tool_step`.
    pub step: &'static str,
    /// The effort the stored request went out at, as the run's records
    /// say; `None` for the model's default, or a run scored before the
    /// records kept it.
    pub recorded: Option<String>,
    /// What Jev answered, when the policy asked; `None` when a lease
    /// held.
    pub asked: Option<Result<Choice, String>>,
    /// The effort the policy sends the request at.
    pub runs_at: Option<String>,
}

/// Replays one run's `timeline` on `picker`'s model. `instructions`
/// stand in for the agent's, which the store does not keep.
pub async fn replay(
    picker: &Picker,
    instructions: &str,
    timeline: &[Entry],
) -> Vec<Decision> {
    let mut decisions = Vec::new();
    if !picker.scores() {
        return decisions;
    }
    let mut transcript: Vec<Message> = Vec::new();
    let mut recorded: Option<String> = None;
    let mut context: Option<Value> = None;
    let mut task = String::new();
    let mut effort: Option<ReasoningEffort> = None;
    let mut lease: Option<Lease> = None;
    let mut starts = false;
    for entry in timeline {
        let message = match entry {
            Entry::Record(body) => {
                if let Some(choice) = Choice::parse(body) {
                    recorded = choice.ran_at().map(str::to_owned);
                }
                continue;
            }
            Entry::Message(message) => message,
        };
        match message {
            Message::User(user) if answered(&transcript) => {
                if let Some(said) = transcript.iter().rev().find_map(said) {
                    context = Some(context_record(&task, &said));
                }
                task = user_text(&user.content);
                starts = true;
            }
            Message::Assistant(_) => {
                let step = if starts { "user_turn" } else { "tool_step" };
                let asks = starts
                    || picker.redecides()
                        && lease_ended(lease, false, &transcript);
                let asked = if !asks {
                    None
                } else if std::mem::take(&mut starts) {
                    let state =
                        message_state(&task, instructions, context.as_ref());
                    Some(picker.ask(state).await)
                } else {
                    let state =
                        step_state(&task, instructions, effort, &transcript);
                    Some(picker.ask(state).await)
                };
                let asked = asked.map(|asked| {
                    asked.map(|asked| {
                        if asked.choice.kind == "chose" {
                            effort = Some(asked.effort);
                        }
                        lease = asked.lease;
                        asked.choice
                    })
                });
                if let Some(Err(_)) = asked {
                    lease = Some(Lease::ToolChain);
                }
                decisions.push(Decision {
                    request: decisions.len() + 1,
                    step,
                    recorded: recorded.clone(),
                    asked,
                    runs_at: effort.map(|effort| effort.as_str().to_owned()),
                });
            }
            _ => {}
        }
        transcript.push(message.clone());
    }
    decisions
}

/// Whether the transcript is empty or ends with a final answer, so a
/// user message starts a new message rather than steering the run.
fn answered(transcript: &[Message]) -> bool {
    match transcript.last() {
        None => true,
        Some(Message::Assistant(reply)) => reply.tool_calls().next().is_none(),
        Some(_) => false,
    }
}

/// The user's words: the last text block, after any context put before
/// them.
fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks
            .iter()
            .rev()
            .find_map(|block| match block {
                InputBlock::Text(text) => Some(text.text.clone()),
                InputBlock::Image(_) => None,
            })
            .unwrap_or_default(),
    }
}

fn said(message: &Message) -> Option<String> {
    crate::said(message).filter(|text| !text.trim().is_empty())
}
