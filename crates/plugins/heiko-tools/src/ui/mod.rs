//! heiko-tools' UI (ADR 0017): the cards of its tools in a run's
//! transcript, each a view tree (ADR 0045). `exec_command` and
//! `write_stdin` show their terminal, or the last lines of their output,
//! and how the command stands as a stamp; `read`, the lines the model
//! got; `edit` and `write`, the diff they made; `ls`, the directory;
//! `find`, the paths; `grep`, the matching lines under their files, what
//! it looked for marked in them;
//! `artifact_read`, how much of the artifact it read.
//!
//! The host builds the plugin's tools on the run's workspace, where they
//! act (`heiko-tools-host`); this UI draws what they return.

pub mod listing;
pub mod listing_card;
pub mod term;
#[cfg(feature = "terminal")]
pub mod term_card;

use std::collections::BTreeMap;

use heiko_ui_kit::{diff, syntax, theme::Tone};
use heiko_ui_plugin::{
    CallData,
    Fold,
    Handle,
    Manifest,
    PluginStatus,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRun, CardView},
    tree::{Ink, Kind, Node, Tone as TreeTone},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use self::listing::DirListing;
use crate::artifact_grant::{ArtifactGrant, ArtifactMetadata, ArtifactRecord};

/// The name heiko-tools goes by in the interface.
pub const NAME: &str = "heiko-tools";

/// heiko-tools with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolsUi;

/// What the UI state keys of the artifacts' last previews start with,
/// before each one's id: the range the host half read, or why not.
pub const PREVIEW: &str = "preview/";

/// Serializable grant fold shared by live and reloaded runs, including phones.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    pub grants: BTreeMap<String, ArtifactGrant>,
    pub error: Option<String>,
}

impl Fold for State {
    type Record = ArtifactRecord;

    fn apply(&mut self, record: ArtifactRecord, _run: &mut dyn RunCx) {
        let ArtifactRecord::ArtifactGrant(grant) = record;
        let id = grant.artifact.id.clone();
        if self
            .grants
            .get(&id)
            .is_some_and(|previous| previous != &grant)
        {
            self.error = Some("Conflicting artifact grants".into());
        } else {
            self.grants.insert(id, grant);
        }
    }
}

/// What a card asks the host half: a range of an artifact the run was
/// granted, to preview it.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Read {
        run: heiko_agent::tool::RunId,
        id: String,
        offset: u64,
        encoding: String,
    },
}

/// What the host half answers an [`Action::Read`]: the range read, or
/// why it could not be.
#[derive(Debug, Serialize, Deserialize)]
pub struct ActionReply {
    pub id: String,
    pub range: Option<Value>,
    pub error: Option<String>,
}

/// Metadata or an explicit absence reason from a finished tool result.
#[derive(Debug, PartialEq, Eq)]
pub enum ArtifactStatus {
    Available {
        metadata: ArtifactMetadata,
        source: String,
        source_complete: Option<bool>,
    },
    Unavailable(String),
}

pub fn artifact_status(data: &CallData) -> Option<ArtifactStatus> {
    let details = match data.result.as_ref()?.details.as_ref() {
        Some(details) => details,
        None => {
            return Some(ArtifactStatus::Unavailable(
                "metadata unavailable".into(),
            ));
        }
    };
    if let Some(artifact) =
        details.get("artifact").filter(|value| !value.is_null())
    {
        let Ok(metadata) = serde_json::from_value(artifact.clone()) else {
            return Some(ArtifactStatus::Unavailable(
                "invalid artifact metadata".into(),
            ));
        };
        return Some(ArtifactStatus::Available {
            metadata,
            source: artifact
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            source_complete: details
                .get("source_complete")
                .and_then(Value::as_bool),
        });
    }
    let reason = details
        .get("artifact_error")
        .and_then(Value::as_str)
        .or_else(|| {
            (details.get("total_bytes").and_then(Value::as_u64) == Some(0))
                .then_some("empty output")
        })
        .unwrap_or("metadata unavailable");
    Some(ArtifactStatus::Unavailable(reason.to_owned()))
}

/// Why a finished call's artifact was not kept, when the tool said so.
/// Provenance is the inspector's; a card mentions the artifact only
/// when keeping it failed.
fn artifact_error(data: &CallData) -> Option<ArtifactStatus> {
    data.result
        .as_ref()
        .and_then(|result| result.details.as_ref())
        .and_then(|details| details.get("artifact_error"))
        .and_then(Value::as_str)
        .map(|error| ArtifactStatus::Unavailable(error.to_owned()))
}

/// What a card or the inspector says of an artifact: what it is and
/// where it came from, or why it is missing.
fn artifact_line(status: &ArtifactStatus) -> Node {
    let text = match status {
        ArtifactStatus::Available {
            metadata,
            source,
            source_complete,
        } => format!(
            "Artifact {} · SHA-256 {} · {} bytes · {} · observed source {}",
            metadata.id,
            metadata.digest,
            metadata.size_bytes,
            source,
            match source_complete {
                Some(true) => "complete",
                Some(false) => "incomplete",
                None => "completion unknown",
            }
        ),
        ArtifactStatus::Unavailable(reason) => {
            format!("Artifact unavailable: {reason}")
        }
    };
    Node::Mono { text }
}

/// The lines a card shows of a `read` before it is opened.
pub const PEEK: usize = 4;

/// What a `read` gave the model: its lines, numbered from `first`, of a
/// file `total` lines long.
#[derive(Debug, PartialEq, Eq)]
pub struct ReadView {
    pub first: usize,
    pub total: usize,
    pub lines: Vec<String>,
}

impl ReadView {
    /// A finished text read's; none for a failed call or an image. A
    /// result without the counts reads as the whole file.
    pub fn of(data: &CallData) -> Option<Self> {
        let result = data.result.as_ref().filter(|result| !result.error)?;
        let details = result.details.as_ref();
        let count = |key| {
            details
                .and_then(|details| details.get(key))
                .and_then(Value::as_u64)
                .map(|n| n as usize)
        };
        if details
            .and_then(|details| details.get("kind"))
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "text")
        {
            return None;
        }
        // The notice the model got after them is not the file's.
        let lines: Vec<String> = result
            .text
            .lines()
            .take(count("returned_lines").unwrap_or(usize::MAX))
            .map(|line| line.replace('\t', "    "))
            .collect();
        Some(Self {
            first: count("offset").unwrap_or(1).max(1),
            total: count("total_lines").unwrap_or(lines.len()),
            lines,
        })
    }

    /// The header's word on it: `lines 120–179 of 612`.
    pub fn label(&self) -> String {
        let shown = self.lines.len();
        let last = (self.first + shown).saturating_sub(1).max(self.first);
        format!(
            "lines {}–{} of {}",
            heiko_ui_kit::format::grouped(self.first),
            heiko_ui_kit::format::grouped(last),
            heiko_ui_kit::format::grouped(self.total)
        )
    }
}

/// The diff a call's result carries, when it carries one.
pub fn diff_of(data: &CallData) -> Option<Vec<diff::DiffLine>> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    let text = result.details.as_ref()?.get("diff")?.as_str()?;
    Some(diff::parse(text))
}

/// What an `exec_command` or `write_stdin` call printed, line by line: the text the model got
/// once it ended, else its output so far.
pub fn output_lines(data: &CallData) -> Vec<String> {
    let text = match &data.result {
        Some(result) => result.text.as_str(),
        None => data.partial.as_deref().unwrap_or_default(),
    };
    text.lines().map(str::to_owned).collect()
}

/// What an ended `exec_command` printed of a test run, as the compare
/// screen's stat: `12 / 14`, failing when fewer passed than ran. A
/// summary such as `14 tests run: 12 passed, 2 failed` says it.
pub fn tests_stat(at: &AtCard) -> Option<PluginStatus> {
    if at.tool != "exec_command" {
        return None;
    }
    at.data.result.as_ref()?;
    let lines = output_lines(&at.data);
    let (total, passed) = lines.iter().rev().find_map(|line| {
        let (before, after) = line.split_once(" tests run: ")?;
        let total = before.split_whitespace().last()?;
        let passed: String =
            after.chars().take_while(char::is_ascii_digit).collect();
        Some((total.to_owned(), passed))
    })?;
    Some(PluginStatus {
        name: "tests".into(),
        tone: if passed == total {
            Tone::Quiet
        } else {
            Tone::Danger
        },
        state: format!("{passed} / {total}"),
    })
}

/// The last `count` lines of `lines`.
fn tail(lines: Vec<String>, count: usize) -> Vec<String> {
    let start = lines.len().saturating_sub(count);
    lines[start..].to_vec()
}

/// The key of the UI state that holds whether a card shows all of its
/// result: `what` is the card's tool.
fn open_key(what: &str, at: &AtCard) -> String {
    format!("{what}-open/{}/{}", at.run.id.0, at.call_id)
}

/// Whether the person opened the card's result.
fn opened(key: &str, view: &ViewCx<'_, ToolsUi>) -> bool {
    view.ui_value(key).and_then(Value::as_bool).unwrap_or(false)
}

/// The button that shows the rest of a result (`label` says how much),
/// or hides it again, by setting the UI state `key`.
pub(crate) fn more(key: &str, open: bool, label: String) -> Node {
    Node::Button {
        label: if open { "show less".to_owned() } else { label },
        action: MORE.into(),
        args: json!({ "key": key, "open": !open }),
        kind: Kind::Secondary,
        icon: None,
        size: Default::default(),
        wide: false,
        detail: None,
    }
}

/// `n noun`, or `n nouns`, the number grouped.
fn counted(n: usize, noun: &str) -> String {
    format!(
        "{} {noun}{}",
        heiko_ui_kit::format::grouped(n),
        if n == 1 { "" } else { "s" }
    )
}

/// The lines a card shows of a diff before it is opened.
pub const DIFF_PEEK: usize = 12;

/// A unified diff down to its first `keep` lines, its headers aside.
pub fn diff_head(diff: &str, keep: usize) -> String {
    let mut kept = Vec::new();
    let mut left = None;
    for line in diff.lines() {
        match &mut left {
            None => {
                if line.starts_with("@@") {
                    left = Some(keep);
                }
            }
            Some(0) => break,
            Some(left) => *left -= 1,
        }
        kept.push(line);
    }
    kept.join("\n")
}

/// A diffstat's five squares: green by the share of lines added, red by
/// the share removed.
pub fn diffstat(added: usize, removed: usize) -> Node {
    let (green, red) = diff::squares(added, removed);
    let squares = |count: usize, tone| Node::Toned {
        text: "■".repeat(count),
        tone,
        mono: true,
    };
    Node::row(
        [
            (green, TreeTone::Good),
            (red, TreeTone::Bad),
            (diff::SQUARES - green - red, TreeTone::Neutral),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, tone)| squares(count, tone)),
    )
}

/// An `edit` or `write` card's body: the diff's first lines, or all of
/// them once opened, as bands, and the button that shows the rest
/// (opening the UI state `key`).
pub fn diff_body(
    text: &str,
    path: Option<&str>,
    key: &str,
    open: bool,
) -> Node {
    let lines = diff::parse(text).len();
    let rest = lines.saturating_sub(DIFF_PEEK);
    let shown = if open || rest == 0 {
        text.to_owned()
    } else {
        diff_head(text, DIFF_PEEK)
    };
    let mut pieces = vec![Node::Diff {
        text: shown,
        path: path.map(str::to_owned),
    }];
    if rest > 0 {
        pieces.push(more(
            key,
            open,
            format!("+ {}", counted(rest, "more line")),
        ));
    }
    Node::stack(pieces)
}

/// How a command stands, as the stamp its card ends with: `exit 0`,
/// `exit 100`, `running`. None for a call that failed before the
/// command ran.
pub fn outcome(data: &CallData) -> Option<Node> {
    let stamp = |label: String, tone| Node::Stamp { label, tone };
    let running = || stamp("running".into(), Ink::Good);
    let Some(result) = &data.result else {
        return Some(running());
    };
    let term = result
        .details
        .as_ref()
        .and_then(|details| details.get("term"));
    let status = term
        .and_then(|term| term.get("status"))
        .and_then(Value::as_str);
    if status == Some("running")
        || (status.is_none() && result.text.contains("is still running: "))
    {
        return Some(running());
    }
    let code = term
        .and_then(|term| term.get("exitCode"))
        .and_then(Value::as_i64)
        .or_else(|| {
            result.text.lines().rev().find_map(|line| {
                line.strip_prefix("Command exited with code ")?
                    .trim()
                    .parse()
                    .ok()
            })
        });
    match code {
        Some(0) => Some(stamp("exit 0".into(), Ink::Good)),
        Some(code) => Some(stamp(format!("exit {code}"), Ink::Bad)),
        None if result.error => None,
        None => Some(stamp("exit 0".into(), Ink::Good)),
    }
}

/// What a command card says beside its stamp: the process a call left
/// running, to write to; for `write_stdin`, how many lines came back.
pub fn command_label(tool: &str, data: &CallData) -> Option<String> {
    let result = data.result.as_ref()?;
    if tool == "write_stdin" {
        let output = result
            .text
            .split_once("\n\n[process")
            .map_or(result.text.as_str(), |(output, _)| output);
        let lines = match output {
            "(no new output)" | "(no output)" => 0,
            output => output.lines().count(),
        };
        return Some(counted(lines, "new line"));
    }
    let term = result.details.as_ref()?.get("term")?;
    (term.get("status").and_then(Value::as_str) == Some("running"))
        .then(|| term.get("process").and_then(Value::as_u64))
        .flatten()
        .map(|process| format!("process {process}"))
}

/// The paths a `find` gave the model, without the notice after them.
pub fn found(data: &CallData) -> Option<Vec<String>> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    if result.text == "No files found matching pattern" {
        return Some(Vec::new());
    }
    let paths = result
        .text
        .split_once("\n\n[")
        .map_or(result.text.as_str(), |(paths, _)| paths);
    Some(paths.lines().map(str::to_owned).collect())
}

/// The paths a card shows of a `find` before it is opened.
pub const FOUND_PEEK: usize = 8;

/// A `find` card's body: its paths on a band, the first few until
/// opened.
pub fn found_body(paths: &[String], key: &str, open: bool) -> Node {
    let rest = paths.len().saturating_sub(FOUND_PEEK);
    let shown = if open {
        paths.len()
    } else {
        paths.len() - rest
    };
    let mut pieces: Vec<Node> = paths[..shown]
        .iter()
        .map(|path| Node::Mono { text: path.clone() })
        .collect();
    if rest > 0 {
        pieces.push(more(key, open, format!("+ {rest} more")));
    }
    Node::Band { children: pieces }
}

/// One line a `grep` gave the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepLine {
    pub path: String,
    pub number: usize,
    pub text: String,
    /// It matched; else it is context around a match.
    pub matched: bool,
}

/// The lines a `grep` gave the model, read back from its text:
/// `path:12: text` for a match, `path-13- text` for context.
pub fn grep_lines(data: &CallData) -> Option<Vec<GrepLine>> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    if result.text == "No matches found" {
        return Some(Vec::new());
    }
    let lines = result
        .text
        .split_once("\n\n[")
        .map_or(result.text.as_str(), |(lines, _)| lines);
    Some(lines.lines().filter_map(grep_line).collect())
}

/// `path:12: text` or `path-13- text`, taken apart at the first
/// separator, number, the same separator and a space: the host always
/// writes that space, so a path like `foo-1-bar.rs` keeps its own
/// dashes and digits. Only a path that itself holds `-N- ` or `:N: `
/// reads back wrong.
fn grep_line(line: &str) -> Option<GrepLine> {
    line.char_indices().find_map(|(at, sep)| {
        let matched = match sep {
            ':' => true,
            '-' => false,
            _ => return None,
        };
        let rest = &line[at + 1..];
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        let text = rest[digits..].strip_prefix(sep)?.strip_prefix(' ')?;
        Some(GrepLine {
            path: line[..at].to_owned(),
            number: rest[..digits].parse().ok()?,
            text: text.to_owned(),
            matched,
        })
    })
}

/// `5 matches in 2 files`.
pub fn grep_label(lines: &[GrepLine]) -> String {
    let matches = lines.iter().filter(|line| line.matched).count();
    if matches == 0 {
        return "no matches".into();
    }
    let mut files: Vec<&str> =
        lines.iter().map(|line| line.path.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    format!(
        "{} {} in {}",
        heiko_ui_kit::format::grouped(matches),
        if matches == 1 { "match" } else { "matches" },
        counted(files.len(), "file")
    )
}

/// What a `grep` looked for, when its matches can be found again
/// without a regex engine: a literal, or literals with `|` between
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needles {
    literals: Vec<String>,
    ignore_case: bool,
}

impl Needles {
    /// The characters that make a pattern more than its own text.
    const META: &str = r"\.^$*+?()[]{}";

    /// From a `grep` call's arguments. None when the pattern is a regex
    /// that says more than literals and `|`.
    pub fn of(args: &Value) -> Option<Self> {
        let pattern = args.get("pattern")?.as_str()?;
        let flag =
            |key: &str| args.get(key).and_then(Value::as_bool).unwrap_or(false);
        let literals: Vec<String> = if flag("literal") {
            vec![pattern.to_owned()]
        } else {
            pattern.split('|').map(str::to_owned).collect()
        };
        let plain =
            flag("literal") || !pattern.chars().any(|c| Self::META.contains(c));
        (plain && literals.iter().all(|literal| !literal.is_empty())).then(
            || Self {
                literals,
                ignore_case: flag("ignoreCase"),
            },
        )
    }

    /// Where `text` holds one of the literals: the first and last byte
    /// of each place, counted from 1, as [`Node::Marked`] takes them.
    /// Without regard to case, only ASCII letters fold.
    pub fn spans(&self, text: &str) -> Vec<(u32, u32)> {
        let bytes = text.as_bytes();
        let found = |at: usize| {
            self.literals.iter().map(String::as_bytes).find(|literal| {
                bytes[at..].get(..literal.len()).is_some_and(|window| {
                    if self.ignore_case {
                        window.eq_ignore_ascii_case(literal)
                    } else {
                        window == *literal
                    }
                })
            })
        };
        let mut spans = Vec::new();
        let mut at = 0;
        while at < bytes.len() {
            match found(at) {
                Some(literal) => {
                    spans.push((at as u32 + 1, (at + literal.len()) as u32));
                    at += literal.len();
                }
                None => at += 1,
            }
        }
        spans
    }
}

/// The lines a card shows of a `grep` before it is opened.
pub const GREP_PEEK: usize = 8;

/// A `grep` card's body, on a band: each file's path over its lines,
/// every line after its number, what `needles` find in a matching line
/// marked, context lines dimmed; the first few until opened.
pub fn grep_body(
    lines: &[GrepLine],
    needles: Option<&Needles>,
    key: &str,
    open: bool,
) -> Node {
    let rest = lines.len().saturating_sub(GREP_PEEK);
    let shown = if open {
        lines.len()
    } else {
        lines.len() - rest
    };
    let width = lines[..shown]
        .iter()
        .map(|line| line.number.to_string().len())
        .max()
        .unwrap_or(1);
    let mut pieces = Vec::new();
    let mut path = None;
    for line in &lines[..shown] {
        if path != Some(&line.path) {
            path = Some(&line.path);
            pieces.push(Node::Toned {
                text: line.path.clone(),
                tone: TreeTone::Neutral,
                mono: true,
            });
        }
        // The number is part of the line, so a long one is cut at the
        // band's edge beside its number, not moved under it.
        let number = format!("{:>width$}  ", line.number);
        let code = line.text.replace('\t', "    ");
        let spans = match needles {
            Some(needles) if line.matched => needles
                .spans(&code)
                .into_iter()
                .map(|(first, last)| {
                    let shift = number.len() as u32;
                    (first + shift, last + shift)
                })
                .collect(),
            _ => Vec::new(),
        };
        let text = format!("{number}{code}");
        pieces.push(if line.matched {
            Node::Marked { text, spans }
        } else {
            Node::Toned {
                text,
                tone: TreeTone::Neutral,
                mono: true,
            }
        });
    }
    if rest > 0 {
        pieces.push(more(
            key,
            open,
            format!("+ {}", counted(rest, "more line")),
        ));
    }
    Node::Band { children: pieces }
}

/// The ticks an `artifact_read` card draws its share on.
pub const SHARE_TICKS: u32 = 21;

/// What an `artifact_read` read: the artifact, and the bytes from
/// `start` up to `end`, from its result's text.
pub fn artifact_range(data: &CallData) -> Option<(String, u64, u64)> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    let rest = result.text.strip_prefix("Read artifact ")?;
    let (id, range) = rest.split_once(" at bytes ")?;
    let (start, end) = range.trim().split_once("..")?;
    Some((id.to_owned(), start.parse().ok()?, end.parse().ok()?))
}

/// An `artifact_read` card: `4.0 KB of 18 KB`, over ticks lit by the
/// share of the artifact read so far. `size` is the artifact's, when the
/// run's grants tell.
pub fn artifact_card(start: u64, end: u64, size: Option<u64>) -> CardView {
    let read = listing::size(end.saturating_sub(start));
    let Some(size) = size.filter(|size| *size > 0) else {
        return CardView {
            label: Some(read),
            ..CardView::default()
        };
    };
    let lit = (end.min(size) * u64::from(SHARE_TICKS)).div_ceil(size) as u32;
    CardView {
        label: Some(format!("{read} of {}", listing::size(size))),
        body: Some(Node::Ticks {
            lit,
            total: SHARE_TICKS,
            tone: Ink::Info,
            marked: Vec::new(),
            labels: Vec::new(),
        }),
        ..CardView::default()
    }
}

/// The tree's name for a language heiko-ui-kit highlights.
fn lang_name(lang: syntax::Lang) -> &'static str {
    use syntax::Lang;
    match lang {
        Lang::Bash => "bash",
        Lang::Go => "go",
        Lang::JavaScript => "javascript",
        Lang::Json => "json",
        Lang::Luau => "luau",
        Lang::Markdown => "markdown",
        Lang::Nix => "nix",
        Lang::Python => "python",
        Lang::Rust => "rust",
        Lang::Toml => "toml",
        Lang::Tsx => "tsx",
        Lang::TypeScript => "typescript",
    }
}

/// The action of the inspector's preview buttons, whose arguments are
/// the [`Action`] to ask the host half.
const ACT: &str = "act";
/// The action of a card's button that shows the rest of its result, or
/// hides it again, with `{ "key": its UI state key, "open": bool }`.
const MORE: &str = "more";

impl UiPlugin for ToolsUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn reply(&self, reply: Value, handle: &Handle) {
        if let Ok(reply) = serde_json::from_value::<ActionReply>(reply) {
            let preview =
                reply.range.unwrap_or_else(|| json!({"error": reply.error}));
            handle.set_ui(format!("{PREVIEW}{}", reply.id), Some(preview));
        }
    }

    fn declaration(&self) -> heiko_ui_plugin::Declaration {
        heiko_ui_plugin::Declaration::new()
            .seams(&[heiko_ui_plugin::Seam::Tools])
            .services(&[
                heiko_ui_plugin::services::names::WORKSPACE_DIR,
                heiko_ui_plugin::services::names::RUN_WAKE,
                heiko_ui_plugin::services::names::RUN_COMMANDS,
                heiko_ui_plugin::services::names::AGENT_REPO,
            ])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::CARD, card)
            .contribute(points::INSPECTOR, |_: &AtRun, view| {
                let state = view.state?;
                let run = view.run?.id.clone();
                let previews: BTreeMap<String, Value> = view
                    .ui_state()
                    .iter()
                    .filter_map(|(key, preview)| {
                        let id = key.strip_prefix(PREVIEW)?;
                        Some((id.to_owned(), preview.clone()))
                    })
                    .collect();
                Some(inspector(state, &run, &previews))
            })
            .contribute(points::CARD_STAT, |at: &AtCard, _| tests_stat(at))
            .contribute_at(points::STATUS, -20, |_: &AtRun, _| {
                Some(PluginStatus {
                    name: NAME.into(),
                    state: "8 tools".into(),
                    tone: Tone::Quiet,
                })
            })
    }

    /// What its trees' buttons ask: the inspector's previews, the rest
    /// of a card's result, and a terminal's full output or its text.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        match action {
            ACT => {
                if let Ok(act) = serde_json::from_value::<Action>(args.clone())
                {
                    view.handle.act(act);
                }
            }
            MORE => {
                if let (Some(key), Some(open)) = (
                    args.get("key").and_then(Value::as_str),
                    args.get("open").and_then(Value::as_bool),
                ) {
                    view.handle.set_ui(key, Some(Value::Bool(open)));
                }
            }
            #[cfg(feature = "terminal")]
            term_card::OPEN => {
                if let Some(path) = args.get("path").and_then(Value::as_str) {
                    view.handle.open(path);
                }
            }
            #[cfg(feature = "terminal")]
            term_card::COPY => {
                if let Some(text) = args.get("text").and_then(Value::as_str) {
                    view.handle.copy(text);
                }
            }
            _ => {}
        }
    }
}

/// The inspector's artifacts: each one the run was granted, what the host
/// half last read of it, and buttons to read on as text or base64.
pub fn inspector(
    state: &State,
    run: &heiko_agent::tool::RunId,
    previews: &BTreeMap<String, Value>,
) -> Node {
    let mut pieces = vec![Node::Heading {
        text: format!("Artifacts · {}", state.grants.len()),
        tone: Default::default(),
    }];
    if let Some(error) = &state.error {
        pieces.push(artifact_line(&ArtifactStatus::Unavailable(error.clone())));
    }
    for grant in state.grants.values() {
        let id = grant.artifact.id.clone();
        let mut entry = vec![artifact_line(&ArtifactStatus::Available {
            metadata: grant.artifact.clone(),
            source: grant.source.clone(),
            source_complete: grant.source_complete,
        })];
        let preview = previews.get(&id);
        let field = |key: &str| preview.and_then(|value| value.get(key));
        let next_offset = field("next_offset").and_then(Value::as_u64);
        let data = field("data").and_then(Value::as_str).unwrap_or("");
        if !data.is_empty() {
            entry.push(Node::Mono {
                text: format!(
                    "Preview at byte {}: {data}",
                    field("offset").and_then(Value::as_u64).unwrap_or(0)
                ),
            });
        }
        if let Some(error) = field("error").and_then(Value::as_str) {
            entry.push(artifact_line(&ArtifactStatus::Unavailable(
                error.to_owned(),
            )));
        }
        let button = |encoding: &str, label: &str| Node::Button {
            label: label.into(),
            action: ACT.into(),
            args: json!(Action::Read {
                run: run.clone(),
                id: id.clone(),
                offset: next_offset.unwrap_or(0),
                encoding: encoding.into(),
            }),
            kind: Kind::Secondary,
            icon: None,
            size: Default::default(),
            wide: false,
            detail: None,
        };
        entry.push(Node::row([
            button("utf8", "Preview text"),
            button("base64", "Preview base64"),
        ]));
        pieces.push(Node::stack(entry));
    }
    Node::stack(pieces)
}

/// A `read` card's body: its first lines, or all of them once opened,
/// highlighted as `lang`, the button that shows the rest (opening the
/// UI state `key`), and why its artifact was not kept, when it was not.
pub fn read_body(
    read: &ReadView,
    key: &str,
    lang: Option<&str>,
    open: bool,
    missing: Option<&ArtifactStatus>,
) -> Node {
    let mut pieces = Vec::new();
    if !read.lines.is_empty() {
        let shown = if open {
            read.lines.len()
        } else {
            read.lines.len().min(PEEK)
        };
        pieces.push(Node::Code {
            lang: lang.map(str::to_owned),
            text: read.lines[..shown].join("\n"),
            first_line: Some(read.first.max(1) as u32),
        });
        let rest = read.lines.len() - read.lines.len().min(PEEK);
        if rest > 0 {
            pieces.push(more(
                key,
                open,
                format!("+ {}", counted(rest, "more line")),
            ));
        }
    }
    if let Some(status) = missing {
        pieces.push(artifact_line(status));
    }
    Node::stack(pieces)
}

/// Output lines, as the model got them, on a band.
fn output(lines: &[String]) -> Node {
    Node::Code {
        lang: None,
        text: lines.join("\n"),
        first_line: None,
    }
}

/// A card of one of its tools.
fn card(at: &AtCard, view: &mut ViewCx<'_, ToolsUi>) -> Option<CardView> {
    let compact = view.compact;
    let data = &at.data;
    match at.tool.as_str() {
        "exec_command" | "write_stdin" => {
            let shape = outcome(data);
            let label = command_label(&at.tool, data);
            #[cfg(feature = "terminal")]
            if let Some(term) = term::TermOutput::of(data, at.cut.is_some()) {
                let id = format!("{}-{}", at.run.id.0, at.call_id);
                let body = term_card::body(&id, &term, at.cut.as_ref());
                return Some(CardView {
                    label,
                    failed: term.failure(),
                    shape,
                    body: Some(body),
                    inset: true,
                    ..CardView::default()
                });
            }
            let lines = match &data.result {
                Some(_) => tail(output_lines(data), 4),
                None => tail(output_lines(data), 6),
            };
            Some(CardView {
                label,
                shape,
                body: (!lines.is_empty()).then(|| output(&lines)),
                ..CardView::default()
            })
        }
        "read" => {
            let read = ReadView::of(data)?;
            let key = open_key("read", at);
            let open = opened(&key, view);
            let lang = data
                .args
                .get("path")
                .and_then(Value::as_str)
                .and_then(syntax::Lang::of_path)
                .map(lang_name);
            // Provenance is the inspector's; the card says only that
            // keeping it failed.
            let missing = artifact_error(data);
            if read.lines.is_empty() && missing.is_none() {
                return Some(CardView {
                    label: Some(read.label()),
                    ..CardView::default()
                });
            }
            let body = read_body(&read, &key, lang, open, missing.as_ref());
            Some(CardView {
                label: Some(read.label()),
                body: Some(body),
                ..CardView::default()
            })
        }
        "edit" | "write" => {
            let result = data.result.as_ref().filter(|result| !result.error)?;
            let details = result.details.as_ref()?;
            let text = details.get("diff")?.as_str()?;
            let (added, removed) = diff::counts(&diff::parse(text));
            let created =
                details.get("created").and_then(Value::as_bool) == Some(true);
            let key = open_key(&at.tool, at);
            let path = data.args.get("path").and_then(Value::as_str);
            Some(CardView {
                label: Some(if created {
                    format!("new · +{added}")
                } else {
                    format!("+{added} −{removed}")
                }),
                shape: Some(diffstat(added, removed)),
                body: Some(diff_body(text, path, &key, opened(&key, view))),
                ..CardView::default()
            })
        }
        listing::TOOL => {
            let result = data.result.as_ref().filter(|result| !result.error)?;
            let listing = DirListing::parse(result.details.as_ref()?)?;
            let key = open_key("ls", at);
            let body =
                listing_card::body(&listing, &key, opened(&key, view), compact);
            Some(CardView {
                label: Some(listing.summary()),
                body,
                ..CardView::default()
            })
        }
        "find" => {
            let paths = found(data)?;
            let key = open_key("find", at);
            Some(CardView {
                label: Some(if paths.is_empty() {
                    "no files".into()
                } else {
                    counted(paths.len(), "file")
                }),
                body: (!paths.is_empty())
                    .then(|| found_body(&paths, &key, opened(&key, view))),
                ..CardView::default()
            })
        }
        "grep" => {
            let lines = grep_lines(data)?;
            let key = open_key("grep", at);
            Some(CardView {
                label: Some(grep_label(&lines)),
                body: (!lines.is_empty()).then(|| {
                    let needles = Needles::of(&data.args);
                    grep_body(
                        &lines,
                        needles.as_ref(),
                        &key,
                        opened(&key, view),
                    )
                }),
                ..CardView::default()
            })
        }
        "artifact_read" => {
            let (id, start, end) = artifact_range(data)?;
            let size = view
                .state
                .and_then(|state| state.grants.get(&id))
                .map(|grant| grant.artifact.size_bytes);
            Some(artifact_card(start, end, size))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use heiko_ui_plugin::CallResult;

    use super::*;

    fn ended(text: &str, details: Option<Value>, error: bool) -> CallData {
        CallData {
            result: Some(CallResult {
                text: text.into(),
                details,
                error,
            }),
            ..CallData::default()
        }
    }

    /// A command's stamp says how it stands: running until it ends and
    /// when its call left it going, then its exit code, good only at 0.
    #[test]
    fn a_command_is_stamped_with_how_it_stands() {
        let stamp = |label: &str, tone| {
            Some(Node::Stamp {
                label: label.into(),
                tone,
            })
        };
        let term = |status: &str, code: Option<i32>| {
            Some(json!({"term": {"status": status, "exitCode": code,
                "process": 3}}))
        };
        assert_eq!(outcome(&CallData::default()), stamp("running", Ink::Good));
        let done = ended("ok", term("exited", Some(0)), false);
        assert_eq!(outcome(&done), stamp("exit 0", Ink::Good));
        let failed = ended("FAIL", term("exited", Some(100)), false);
        assert_eq!(outcome(&failed), stamp("exit 100", Ink::Bad));
        let going = ended("so far", term("running", None), false);
        assert_eq!(outcome(&going), stamp("running", Ink::Good));
        assert_eq!(
            command_label("exec_command", &going).as_deref(),
            Some("process 3")
        );
        assert_eq!(command_label("exec_command", &done), None);
        // Without a terminal, the text says it.
        let text = ended("x\n\nCommand exited with code 2", None, true);
        assert_eq!(outcome(&text), stamp("exit 2", Ink::Bad));
        assert_eq!(outcome(&ended("no such workdir", None, true)), None);
        assert_eq!(
            command_label("write_stdin", &ended("a\nb\nc", None, false))
                .as_deref(),
            Some("3 new lines")
        );
    }

    /// A grep line as the host writes it: `path:12: text` for a match,
    /// `path-13- text` for context.
    fn rendered(line: &GrepLine) -> String {
        let sep = if line.matched { ':' } else { '-' };
        format!("{}{sep}{}{sep} {}", line.path, line.number, line.text)
    }

    /// Whatever lines the host wrote, under paths made of digits, dashes,
    /// dots, slashes and colons, and with or without its notice after
    /// them, read back as those lines.
    #[hegel::test(test_cases = 500)]
    fn a_grep_reads_back_the_lines_the_host_wrote(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let lines: Vec<GrepLine> = tc
            .draw(
                gs::vecs(gs::tuples!(
                    gs::text().min_size(1).max_size(12).alphabet("ab19-./:_"),
                    gs::integers::<usize>().min_value(1).max_value(100_000),
                    gs::text().max_size(12).alphabet("x1 -:[]\t"),
                    gs::booleans(),
                ))
                .min_size(1)
                .max_size(6),
            )
            .into_iter()
            .map(|(path, number, text, matched)| GrepLine {
                path,
                number,
                text,
                matched,
            })
            .collect();
        let mut text =
            lines.iter().map(rendered).collect::<Vec<_>>().join("\n");
        if tc.draw(gs::booleans()) {
            text.push_str(
                "\n\n[100 matches limit reached. Use limit=200 for more, or refine pattern]",
            );
        }
        assert_eq!(grep_lines(&ended(&text, None, false)), Some(lines));
    }

    /// A path with a dash, a number and a dash in it is still the path:
    /// the line's own separators are the ones before its space.
    #[test]
    fn a_path_with_dashed_digits_is_not_a_context_line() {
        let data = ended("foo-1-bar.rs:3: x\nsrc/a.rs:7: ", None, false);
        assert_eq!(
            grep_lines(&data),
            Some(vec![
                GrepLine {
                    path: "foo-1-bar.rs".into(),
                    number: 3,
                    text: "x".into(),
                    matched: true,
                },
                GrepLine {
                    path: "src/a.rs".into(),
                    number: 7,
                    text: String::new(),
                    matched: true,
                },
            ])
        );
    }

    /// The label counts matches and files; no matches and a failed grep
    /// read back as nothing and as no lines.
    #[test]
    fn a_grep_label_counts_matches_and_files() {
        let line = |path: &str, matched| GrepLine {
            path: path.into(),
            number: 1,
            text: String::new(),
            matched,
        };
        let lines = [
            line("src/a.rs", true),
            line("src/a.rs", false),
            line("src/b.rs", true),
        ];
        assert_eq!(grep_label(&lines), "2 matches in 2 files");
        assert_eq!(grep_label(&lines[..1]), "1 match in 1 file");
        assert_eq!(
            grep_lines(&ended("No matches found", None, false)),
            Some(Vec::new())
        );
        assert_eq!(grep_label(&[]), "no matches");
        assert_eq!(grep_lines(&ended("bad pattern", None, true)), None);
    }

    /// A grep for literals marks them in its matching lines, after each
    /// line's number; a pattern that says more marks nothing.
    #[test]
    fn a_grep_for_literals_marks_them() {
        let needles =
            Needles::of(&json!({ "pattern": "retry_after|RetryPolicy" }))
                .expect("literals");
        assert_eq!(
            needles.spans("policy: &RetryPolicy, retry_after"),
            [(10, 20), (23, 33)]
        );
        assert!(needles.spans("retrypolicy").is_empty());
        let folded = Needles::of(
            &json!({ "pattern": "retrypolicy", "ignoreCase": true }),
        )
        .expect("a literal");
        assert_eq!(folded.spans("é RetryPolicy"), [(4, 14)]);
        // As a literal, the pattern's characters are its own.
        let literal =
            Needles::of(&json!({ "pattern": "a.b(|", "literal": true }))
                .expect("a literal");
        assert_eq!(literal.spans("xa.b(|"), [(2, 6)]);
        for pattern in ["a.b", "fn \\w+", "(a|b)", "a|", "", "^use", "x*"] {
            assert_eq!(
                Needles::of(&json!({ "pattern": pattern })),
                None,
                "{pattern}"
            );
        }
        assert_eq!(Needles::of(&json!({})), None);

        let line = |number, text: &str, matched| GrepLine {
            path: "src/a.rs".into(),
            number,
            text: text.into(),
            matched,
        };
        let lines = [
            line(5, "pub struct RetryPolicy {", true),
            line(12, "    max: u32,", false),
        ];
        let Node::Band { children } =
            grep_body(&lines, Some(&needles), "key", false)
        else {
            panic!("a band");
        };
        assert_eq!(
            children[1],
            Node::Marked {
                text: " 5  pub struct RetryPolicy {".into(),
                spans: vec![(16, 26)],
            }
        );
        assert!(matches!(&children[2], Node::Toned { text, .. }
            if text == "12      max: u32,"));
        // Without literals to find, the line is there unmarked.
        let Node::Band { children } = grep_body(&lines, None, "key", false)
        else {
            panic!("a band");
        };
        assert!(matches!(&children[1], Node::Marked { spans, .. }
            if spans.is_empty()));
    }

    /// A long diff shows its first lines until opened, under its headers.
    #[test]
    fn a_long_diff_opens_to_every_line() {
        let body: String = (0..20).map(|n| format!("+line {n}\n")).collect();
        let text = format!("--- a\n+++ b\n@@ -0,0 +1,20 @@\n{body}");
        let shown = |open| match diff_body(&text, None, "key", open) {
            Node::Stack { children } => match &children[0] {
                Node::Diff { text, .. } => diff::parse(text).len(),
                other => panic!("a diff: {other:?}"),
            },
            other => panic!("a stack: {other:?}"),
        };
        assert_eq!(shown(false), DIFF_PEEK);
        assert_eq!(shown(true), 20);
    }

    /// The share of an artifact read lights its ticks; with no size
    /// known, the card says only how much was read.
    #[test]
    fn an_artifact_read_lights_its_share() {
        let card = artifact_card(0, 4096, Some(18 * 1024));
        assert_eq!(card.label.as_deref(), Some("4.0 KB of 18 KB"));
        let Some(Node::Ticks { lit, total, .. }) = card.body else {
            panic!("ticks");
        };
        assert_eq!((lit, total), (5, SHARE_TICKS));
        assert_eq!(artifact_card(0, 10, None).body, None);
        assert_eq!(
            artifact_range(&ended(
                "Read artifact art-7c21 at bytes 0..4096",
                None,
                false
            )),
            Some(("art-7c21".into(), 0, 4096))
        );
    }
}
