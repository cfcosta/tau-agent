//! What a script leaves: its output, its calls and its writes, and the
//! result the model reads.

use std::{path::PathBuf, time::Duration};

use serde_json::{Value, json};
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
            "store": self.store.as_ref().map(Writes::to_json),
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
        content.extend(budget(items, max_output_tokens, spill));
        Rendered {
            content,
            is_error: self.is_error(),
            details: self.details(),
        }
    }
}

/// Tokens in `text`, at four characters a token.
pub fn tokens(text: &str) -> u64 {
    (text.chars().count() as u64).div_ceil(4)
}

/// `text` cut in the middle to fit `max_tokens`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cut<'a> {
    Whole,
    Cut {
        head: &'a str,
        tail: &'a str,
        original_tokens: u64,
        removed_tokens: u64,
    },
}

/// Cuts `text` to `max_tokens`, keeping half the budget's characters
/// from each end, at character boundaries.
pub fn cut(text: &str, max_tokens: u64) -> Cut<'_> {
    let original_tokens = tokens(text);
    if original_tokens <= max_tokens {
        return Cut::Whole;
    }
    let chars = text.chars().count();
    let keep = (max_tokens.saturating_mul(4)).min(chars as u64) as usize;
    let head_chars = keep / 2;
    let tail_chars = keep - head_chars;
    let head_end = text
        .char_indices()
        .nth(head_chars)
        .map_or(text.len(), |(at, _)| at);
    let tail_start = if tail_chars == 0 {
        text.len()
    } else {
        text.char_indices()
            .nth(chars - tail_chars)
            .map_or(text.len(), |(at, _)| at)
    };
    let removed = &text[head_end..tail_start];
    Cut::Cut {
        head: &text[..head_end],
        tail: &text[tail_start..],
        original_tokens,
        removed_tokens: tokens(removed),
    }
}

/// The text items of `items`, cut to `max_tokens` if they pass it; a
/// cut merges them into one item, with the images after it. `save`
/// keeps the full text and returns where.
pub fn budget(
    items: Vec<Item>,
    max_tokens: u64,
    save: impl FnOnce(&str) -> Result<PathBuf, String>,
) -> Vec<Item> {
    let total: u64 = items
        .iter()
        .map(|item| match item {
            Item::Text(text) => tokens(text),
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

/// `full` as the truncation notice, head and tail.
pub fn truncated(
    full: &str,
    max_tokens: u64,
    save: impl FnOnce(&str) -> Result<PathBuf, String>,
) -> String {
    let Cut::Cut {
        head,
        tail,
        original_tokens,
        removed_tokens,
    } = cut(full, max_tokens)
    else {
        return full.to_owned();
    };
    let lines = full.lines().count().max(1);
    let saved = match save(full) {
        Ok(path) => format!(
            "[Full output: {} (read it with offset/limit)]",
            path.display()
        ),
        Err(error) => format!("[Full output could not be saved: {error}]"),
    };
    format!(
        "Warning: truncated output (original token count: \
         {original_tokens})\nTotal output lines: {lines}\n\n{head}…\
         {removed_tokens} tokens truncated…{tail}\n\n{saved}"
    )
}

/// Writes `text` to a new `$TMPDIR/tau-codemode-<hex>.txt`, readable by
/// its owner only.
pub fn spill(text: &str) -> Result<PathBuf, String> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let path = std::env::temp_dir().join(format!("tau-codemode-{hex}.txt"));
    let write = || {
        use std::{io::Write as _, os::unix::fs::OpenOptionsExt as _};
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?
            .write_all(text.as_bytes())
    };
    write().map_err(|error| error.to_string())?;
    Ok(path)
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
