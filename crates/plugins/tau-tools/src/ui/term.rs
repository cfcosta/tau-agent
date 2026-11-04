//! A command's terminal, from the `term` details of its updates and
//! result (`docs/reference/tools.md`, "exec_command and write_stdin").

use serde_json::Value;
use tau_ui_plugin::CallData;

/// The size tool output runs at, which a command's terminal has until
/// its result says otherwise: 120 columns by 40 rows.
pub const COLS: u16 = 120;
pub const ROWS: u16 = 40;

/// What one call of `exec_command` or `write_stdin` saw of a command's
/// terminal, from the `term` details of its updates and result
/// (`docs/reference/tools.md`, "exec_command and write_stdin").
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
    /// How the call ended; `None` while it runs.
    pub end: Option<TermEnd>,
    /// The text the model got: the result's text, pruned or not.
    pub seen: String,
    /// Whether a plugin cut what the model got.
    pub cut: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermEnd {
    pub status: TermStatus,
    /// The exit code; `None` while the command still runs.
    pub exit_code: Option<i32>,
    /// Whether `bytes` is a VT snapshot of the final screen, rather
    /// than the whole raw output.
    pub snapshot: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermStatus {
    Exited,
    /// The call returned with the command still running, to go on with
    /// `write_stdin`.
    Running,
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

    /// The terminal a call drew: its result's replay once it ended,
    /// else the chunks its updates carried, folded in order; `None` for
    /// a command that ran without one, or has not written yet. `cut`
    /// says whether a plugin cut what the model got.
    pub fn of(data: &CallData, cut: bool) -> Option<Self> {
        if let Some(result) = &data.result {
            let term = result.details.as_ref()?.get("term")?;
            let mut output = Self::from_result(term, result.text.clone())?;
            output.cut = cut;
            return Some(output);
        }
        let mut output = Self::new(COLS, ROWS);
        for update in &data.updates {
            if let Some(term) = update.get("term") {
                output.push_chunk(term);
            }
        }
        (output.next_seq > 0).then_some(output)
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
            "running" => TermStatus::Running,
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

    /// What the card's header says of how it ended: `exit 100`; `None`
    /// for a clean exit, and for a command still running.
    pub fn failure(&self) -> Option<String> {
        let end = self.end?;
        match end.status {
            TermStatus::Exited => match end.exit_code {
                Some(0) => None,
                Some(code) => Some(format!("exit {code}")),
                None => Some("exited".into()),
            },
            TermStatus::Running => None,
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
