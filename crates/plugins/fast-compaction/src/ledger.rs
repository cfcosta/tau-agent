//! The decisions so far, and what they make of a transcript.
//!
//! Ported from `joelhooks/pi-fast-jev-compaction` (`src/ledger.ts`; see
//! `THIRD_PARTY_NOTICES.md`).

use std::collections::{BTreeMap, BTreeSet};

use tau_ai::message::{
    AssistantBlock,
    InputBlock,
    Message,
    TextContent,
    ToolResultMessage,
};

use crate::{
    decide::{Action, Decision},
    state::block_text,
};

/// The note that marks a result this plugin cut, and its prefix, which
/// keeps a cut result from being cut again.
const TRUNCATED: &str = "[fast-compaction truncated ";

/// Decisions by tool call id. A call's decision only ever escalates:
/// keep, then drop its result, then drop the call.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger(BTreeMap<String, Decision>);

impl Ledger {
    pub fn from_decisions(
        decisions: impl IntoIterator<Item = Decision>,
    ) -> Self {
        let mut ledger = Self::default();
        ledger.merge(decisions);
        ledger
    }

    /// Adds `decisions`, each replacing an earlier one only when it
    /// escalates it.
    pub fn merge(&mut self, decisions: impl IntoIterator<Item = Decision>) {
        for decision in decisions {
            let escalates = self
                .0
                .get(&decision.call_id)
                .is_none_or(|previous| decision.action > previous.action);
            if escalates {
                self.0.insert(decision.call_id.clone(), decision);
            }
        }
    }

    pub fn decisions(&self) -> impl Iterator<Item = &Decision> {
        self.0.values()
    }

    fn action(&self, call_id: &str) -> Action {
        self.0
            .get(call_id)
            .map_or(Action::Keep, |decision| decision.action)
    }

    /// `transcript` with the decisions applied: dropped calls and their
    /// results removed, dropped results cut to `head_chars` characters and
    /// a note. An assistant message whose calls were all dropped, leaving
    /// no text, goes too (only thinking would be left). Everything else is
    /// left as it is, in order: unlike pi, a message holding only thinking
    /// that nothing was pruned from stays.
    pub fn apply(
        &self,
        transcript: &[Message],
        head_chars: usize,
    ) -> Vec<Message> {
        let mut kept = Vec::with_capacity(transcript.len());
        let mut surviving: BTreeSet<String> = BTreeSet::new();
        for message in transcript {
            let Message::Assistant(assistant) = message else {
                kept.push(message.clone());
                continue;
            };
            let content: Vec<AssistantBlock> = assistant
                .content
                .iter()
                .filter(|block| match block {
                    AssistantBlock::ToolCall(call) => {
                        self.action(&call.id) != Action::DropCall
                    }
                    _ => true,
                })
                .cloned()
                .collect();
            let substantive = content.iter().any(|block| {
                matches!(
                    block,
                    AssistantBlock::Text(_) | AssistantBlock::ToolCall(_)
                )
            });
            let pruned = content.len() != assistant.content.len();
            if pruned && !substantive {
                continue;
            }
            for block in &content {
                if let AssistantBlock::ToolCall(call) = block {
                    surviving.insert(call.id.clone());
                }
            }
            let mut assistant = assistant.clone();
            assistant.content = content;
            kept.push(Message::Assistant(assistant));
        }
        kept.into_iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => {
                    if !surviving.contains(&result.tool_call_id) {
                        return None;
                    }
                    match self.action(&result.tool_call_id) {
                        Action::Keep => Some(Message::ToolResult(result)),
                        Action::DropResult => {
                            Some(Message::ToolResult(cut(result, head_chars)))
                        }
                        Action::DropCall => None,
                    }
                }
                other => Some(other),
            })
            .collect()
    }
}

/// A result cut to its first `head_chars` characters and a note, with
/// its images dropped. A result already cut, or short enough that
/// cutting would save little, is left as it is.
fn cut(result: ToolResultMessage, head_chars: usize) -> ToolResultMessage {
    let text = block_text(&result.content);
    let has_images = result
        .content
        .iter()
        .any(|block| matches!(block, InputBlock::Image(_)));
    let length = text.chars().count();
    if text.contains(TRUNCATED) || (!has_images && length <= head_chars + 120) {
        return result;
    }
    let head: String = text.chars().take(head_chars).collect();
    let head = if head_chars > 0 {
        format!("{head}\n")
    } else {
        head
    };
    let note = format!(
        "{TRUNCATED}{} chars of this tool result{}; re-run the tool if needed]",
        length.saturating_sub(head_chars),
        if result.is_error { " (error)" } else { "" }
    );
    ToolResultMessage {
        content: vec![InputBlock::Text(TextContent {
            text: format!("{head}{note}"),
            text_signature: None,
        })],
        ..result
    }
}

/// The size of a transcript, for the reduction ratio: its JSON length.
pub fn measure(transcript: &[Message]) -> usize {
    serde_json::to_string(transcript)
        .expect("messages serialize")
        .len()
}
