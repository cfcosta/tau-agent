//! What a script leaves: its output, its calls and its writes, and the
//! result the model reads.

use std::{path::PathBuf, time::Duration};

use serde_json::{Value, json};
use tau_agent::output::{Spill, estimate_tokens, truncated};
use tau_ai::message::Usage;

use crate::{image::Image, store::Writes};

/// The most call rows a result keeps.
pub const MAX_CALL_ROWS: usize = 256;

/// A row's `args` preview is cut at this many characters.
pub const MAX_ARGS_CHARS: usize = 200;

/// A row's `error` is cut at this many characters.
pub const MAX_ERROR_CHARS: usize = 500;

/// One output item, in the order the script made it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Text(String),
    Image(Image),
}

/// How a nested call went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    /// Still running; a finished script reports these as cancelled.
    Running,
    Ok,
    Error,
    /// The script ended, timed out or was cancelled first.
    Cancelled,
}

impl CallStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    /// The status [`Self::as_str`] names.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "running" => Self::Running,
            "ok" => Self::Ok,
            "error" => Self::Error,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

/// One nested call: a tool call, or a Jev request (`jev.noul`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct CallRow {
    pub id: String,
    pub name: String,
    /// Compact JSON, cut at [`MAX_ARGS_CHARS`].
    pub args: String,
    pub status: CallStatus,
    pub ms: u64,
    /// Cut at [`MAX_ERROR_CHARS`].
    pub error: Option<String>,
    /// US dollars, for calls that cost something (Jev).
    pub cost: Option<f64>,
}

impl CallRow {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "args": self.args,
            "status": self.status.as_str(),
            "ms": self.ms,
            "error": self.error,
            "cost": self.cost,
        })
    }
}

/// Why a script failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// A Lua error, with its line where Luau gave one.
    Error(String),
    TimedOut {
        timeout_ms: u64,
    },
    Cancelled,
    /// The VM could not be set up.
    Sandbox(String),
}

impl Failure {
    /// The failure's first line in the result.
    pub fn head(&self) -> String {
        match self {
            Self::Error(message) => message.clone(),
            Self::TimedOut { timeout_ms } => format!(
                "Script timed out: it ran past its timeout_ms of {timeout_ms} \
                 ms."
            ),
            Self::Cancelled => {
                "Script cancelled: the run was cancelled before the script \
                 finished."
                    .into()
            }
            Self::Sandbox(message) => {
                format!("Script sandbox failed: {message}")
            }
        }
    }
}

/// A finished script.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub failure: Option<Failure>,
    pub wall: Duration,
    pub items: Vec<Item>,
    /// At most [`MAX_CALL_ROWS`], in the order calls started.
    pub calls: Vec<CallRow>,
    /// Every call the script made, rows or not.
    pub calls_total: usize,
    /// The script's store writes; `None` when it failed.
    pub store: Option<Writes>,
    /// Jev's usage, summed.
    pub usage: Usage,
}

/// The result the model reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub content: Vec<Item>,
    pub is_error: bool,
    pub details: Value,
}

impl Outcome {
    pub fn is_error(&self) -> bool {
        self.failure.is_some()
    }

    /// The last item of a failed script's output.
    pub fn failure_text(&self) -> Option<String> {
        let failure = self.failure.as_ref()?;
        let summary = if self.calls.is_empty() {
            "No tool calls were made.".to_owned()
        } else {
            let mut list = self
                .calls
                .iter()
                .map(|row| format!("{} ({})", row.name, row.status.as_str()))
                .collect::<Vec<_>>()
                .join(", ");
            let more = self.calls_total.saturating_sub(self.calls.len());
            if more > 0 {
                list.push_str(&format!(", and {more} more"));
            }
            format!(
                "Tool calls made before the failure (they are not undone): \
                 {list}"
            )
        };
        Some(format!("Script error:\n{}\n\n{summary}", failure.head()))
    }

    /// The details: `{ calls, complete, store, usage, wall_ms }`.
    pub fn details(&self) -> Value {
        json!({
            "calls": self.calls.iter().map(CallRow::to_json).collect::<Vec<_>>(),
            "complete": self.calls_total <= self.calls.len(),
            "store": self.store,
            "usage": self.usage,
            "wall_ms": self.wall.as_millis() as u64,
        })
    }

    /// The result, with the text cut to `max_output_tokens`. A cut
    /// writes the full text to a file under `$TMPDIR`.
    pub fn render(&self, max_output_tokens: u64) -> Rendered {
        let header = format!(
            "{}\nWall time {:.1} seconds\nOutput:\n",
            if self.is_error() {
                "Script failed"
            } else {
                "Script completed"
            },
            self.wall.as_secs_f64()
        );
        let mut items = self.items.clone();
        if let Some(text) = self.failure_text() {
            items.push(Item::Text(text));
        }
        let mut content = vec![Item::Text(header)];
        content.extend(budget(items, max_output_tokens, |full| {
            Spill::temp("tau-codemode").write(full.as_bytes(), "txt")
        }));
        Rendered {
            content,
            is_error: self.is_error(),
            details: self.details(),
        }
    }
}

/// The text items of `items`, cut to `max_tokens` if they pass it; a
/// cut merges them into one item, with the images after it. `save`
/// keeps the full text and returns where.
pub fn budget(
    items: Vec<Item>,
    max_tokens: u64,
    save: impl FnOnce(&str) -> std::io::Result<PathBuf>,
) -> Vec<Item> {
    let total: u64 = items
        .iter()
        .map(|item| match item {
            Item::Text(text) => estimate_tokens(text),
            Item::Image(_) => 0,
        })
        .sum();
    if total <= max_tokens {
        return items;
    }
    let mut texts = Vec::new();
    let mut images = Vec::new();
    for item in items {
        match item {
            Item::Text(text) => texts.push(text),
            Item::Image(image) => images.push(Item::Image(image)),
        }
    }
    let full = texts.join("\n");
    let mut out = vec![Item::Text(truncated(&full, max_tokens, save))];
    out.extend(images);
    out
}

/// `text` cut to `max` characters, with `…` in place of the rest.
pub fn preview(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
