//! A `bash` command's terminal, from the `term` details of its updates
//! and result (`docs/reference/tools.md`, "bash: terminal mode").

use serde_json::Value;

/// A command's terminal, from the `term` details of `bash`'s updates and
/// result (`docs/reference/tools.md`, "bash: terminal mode").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermOutput {
    pub cols: u16,
    pub rows: u16,
    /// The raw output: the chunks so far while the command runs, the
    /// result's replay once it ends. Written to a new `cols`×`rows`
    /// terminal, it draws the screen. Shared, so cloning a card is cheap.
    pub bytes: std::sync::Arc<Vec<u8>>,
    /// The `seq` the next chunk must carry.
    pub next_seq: u64,
    /// How the command ended; `None` while it runs.
    pub end: Option<TermEnd>,
    /// The text the model got: the result's text, pruned or not.
    pub seen: String,
    /// Whether a plugin cut what the model got.
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermEnd {
    pub status: TermStatus,
    /// The exit code; `None` on a timeout or a cancel.
    pub exit_code: Option<i32>,
    /// Whether `bytes` is a VT snapshot of the final screen, rather
    /// than the whole raw output.
    pub snapshot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermStatus {
    Exited,
    TimedOut,
    Cancelled,
}

impl TermOutput {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols,
            rows,
            bytes: Default::default(),
            next_seq: 0,
            end: None,
            seen: String::new(),
            cut: false,
        }
    }

    /// `120×40`.
    pub fn size_label(&self) -> String {
        format!("{}×{}", self.cols, self.rows)
    }

    /// The model's text, line by line; when a plugin cut it, with its
    /// marks read back: its header and footer, and each `[N lines
    /// omitted]`.
    pub fn seen_lines(&self) -> Vec<SeenLine> {
        let lines: Vec<&str> = self.seen.lines().collect();
        let pruned = self.cut;
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                let omitted = line
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(" lines omitted]"))
                    .and_then(|count| count.parse().ok());
                match omitted {
                    Some(count) if pruned => SeenLine::Omitted(count),
                    _ if pruned
                        && (index == 0
                            || (index + 1 == lines.len()
                                && line.starts_with("[full output: "))) =>
                    {
                        SeenLine::Note((*line).to_owned())
                    }
                    _ => SeenLine::Text((*line).to_owned()),
                }
            })
            .collect()
    }

    /// Takes one progress update's `term` details: a chunk, in order.
    /// Out-of-order or repeated chunks are dropped.
    pub fn push_chunk(&mut self, term: &Value) {
        let (Some(seq), Some(bytes)) = (
            term.get("seq").and_then(Value::as_u64),
            term.get("bytes").and_then(Value::as_str).and_then(decode),
        ) else {
            return;
        };
        if seq == self.next_seq && self.end.is_none() {
            std::sync::Arc::make_mut(&mut self.bytes).extend_from_slice(&bytes);
            self.next_seq += 1;
        }
    }

    /// A result's `term` details: how the command ended and its replay.
    pub fn from_result(term: &Value, seen: String) -> Option<Self> {
        let size = |key: &str| {
            term.get(key)
                .and_then(Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
        };
        let status = match term.get("status").and_then(Value::as_str)? {
            "exited" => TermStatus::Exited,
            "timedOut" => TermStatus::TimedOut,
            "cancelled" => TermStatus::Cancelled,
            _ => return None,
        };
        Some(Self {
            cols: size("cols")?,
            rows: size("rows")?,
            bytes: std::sync::Arc::new(
                term.get("bytes").and_then(Value::as_str).and_then(decode)?,
            ),
            next_seq: term.get("chunks").and_then(Value::as_u64).unwrap_or(0),
            end: Some(TermEnd {
                status,
                exit_code: term
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .and_then(|code| i32::try_from(code).ok()),
                snapshot: term.get("replay").and_then(Value::as_str)
                    == Some("snapshot"),
            }),
            seen,
            cut: false,
        })
    }

    /// What the card's header says of how it ended: `exit 100`, `timed
    /// out`, `cancelled`; `None` for a clean exit.
    pub fn failure(&self) -> Option<String> {
        let end = self.end?;
        match end.status {
            TermStatus::Exited => match end.exit_code {
                Some(0) => None,
                Some(code) => Some(format!("exit {code}")),
                None => Some("exited".into()),
            },
            TermStatus::TimedOut => Some("timed out".into()),
            TermStatus::Cancelled => Some("cancelled".into()),
        }
    }
}

/// A line of the text the model saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeenLine {
    Text(String),
    /// A run of lines the cut left out.
    Omitted(usize),
    /// The cut's header or footer.
    Note(String),
}

fn decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(text).ok()
}
