//! tau-tools' UI (ADR 0017): the cards of its tools in a run's
//! transcript, each a view tree (ADR 0045). `exec_command` and
//! `write_stdin` show their terminal, or the last lines of their output;
//! `read`, the lines the model got; `edit` and `write`, the diff they
//! made; `ls`, the directory.
//!
//! The host builds the plugin's tools on the run's workspace, where they
//! act (`tau-tools-host`); this UI draws what they return.

pub mod listing;
pub mod listing_card;
pub mod term;
#[cfg(feature = "terminal")]
pub mod term_card;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ui_kit::{diff, syntax, theme::Tone};
use tau_ui_plugin::{
    CallData,
    Fold,
    Manifest,
    PluginStatus,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRun, CardView},
    tree::{Kind, Node},
};

use self::listing::DirListing;
use crate::artifact_grant::{ArtifactGrant, ArtifactMetadata, ArtifactRecord};

/// The name tau-tools goes by in the interface.
pub const NAME: &str = "tau-tools";

/// tau-tools with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolsUi;

/// What the host half answered in one window: each artifact's last
/// preview, by its id.
#[derive(Default)]
pub struct Ui {
    pub previews: BTreeMap<String, Value>,
}

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
        run: tau_agent::tool::RunId,
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

    /// The header's word on it: `71 lines`, or `120–179 of 612`.
    pub fn label(&self) -> String {
        let shown = self.lines.len();
        if self.first == 1 && shown == self.total {
            return match shown {
                1 => "1 line".into(),
                n => format!("{} lines", tau_ui_kit::format::grouped(n)),
            };
        }
        let last = (self.first + shown).saturating_sub(1).max(self.first);
        format!(
            "{}–{} of {}",
            tau_ui_kit::format::grouped(self.first),
            tau_ui_kit::format::grouped(last),
            tau_ui_kit::format::grouped(self.total)
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

/// The last `count` lines of `lines`.
fn tail(lines: Vec<String>, count: usize) -> Vec<String> {
    let start = lines.len().saturating_sub(count);
    lines[start..].to_vec()
}

/// The key of the UI state that holds whether a `read` card shows all
/// its lines.
fn read_key(at: &AtCard) -> String {
    format!("read-open/{}/{}", at.run.id.0, at.call_id)
}

/// The tree's name for a language tau-ui-kit highlights.
fn lang_name(lang: syntax::Lang) -> &'static str {
    use syntax::Lang;
    match lang {
        Lang::Bash => "bash",
        Lang::Go => "go",
        Lang::JavaScript => "javascript",
        Lang::Json => "json",
        Lang::Luau => "luau",
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
/// The action of a `read` card's button that shows its other lines, or
/// hides them again.
const MORE: &str = "more";

impl UiPlugin for ToolsUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    fn reply(
        &self,
        ui: &mut Self::Ui,
        reply: Value,
        cx: &mut gpui::Context<Self::Ui>,
    ) {
        if let Ok(reply) = serde_json::from_value::<ActionReply>(reply) {
            ui.previews.insert(
                reply.id,
                reply.range.unwrap_or_else(|| json!({"error": reply.error})),
            );
            cx.notify();
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::CARD, card)
            .contribute(points::INSPECTOR, |_: &AtRun, view| {
                let state = view.state?;
                let run = view.run?.id.clone();
                let previews = view.ui.read(view.cx).previews.clone();
                let tree = inspector(state, &run, &previews);
                let handle = view.handle.clone();
                Some(view.tree(
                    &format!("tools-inspector-{}", run.0),
                    &tree,
                    move |action, args, cx| {
                        if action != ACT {
                            return;
                        }
                        if let Ok(act) =
                            serde_json::from_value::<Action>(args.clone())
                        {
                            handle.act(act, cx);
                        }
                    },
                ))
            })
            .contribute_at(points::STATUS, -20, |_: &AtRun, _| {
                Some(PluginStatus {
                    name: NAME.into(),
                    state: "8 tools".into(),
                    tone: Tone::Quiet,
                })
            })
    }
}

/// The inspector's artifacts: each one the run was granted, what the host
/// half last read of it, and buttons to read on as text or base64.
pub fn inspector(
    state: &State,
    run: &tau_agent::tool::RunId,
    previews: &BTreeMap<String, Value>,
) -> Node {
    let mut pieces = vec![Node::Heading {
        text: format!("Artifacts · {}", state.grants.len()),
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
/// highlighted as `lang`, the button that shows the rest, and why its
/// artifact was not kept, when it was not.
pub fn read_body(
    read: &ReadView,
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
        });
        let rest = read.lines.len() - read.lines.len().min(PEEK);
        if rest > 0 {
            pieces.push(Node::Button {
                label: if open {
                    "show less".to_owned()
                } else {
                    format!(
                        "+ {} more {}",
                        tau_ui_kit::format::grouped(rest),
                        if rest == 1 { "line" } else { "lines" }
                    )
                },
                action: MORE.into(),
                args: json!(!open),
                kind: Kind::Secondary,
                icon: None,
            });
        }
    }
    if let Some(status) = missing {
        pieces.push(artifact_line(status));
    }
    Node::stack(pieces)
}

/// Output lines, as the model got them.
fn output(lines: &[String]) -> Node {
    Node::Code {
        lang: None,
        text: lines.join("\n"),
    }
}

/// A card of one of its tools.
fn card(at: &AtCard, view: &mut ViewCx<'_, ToolsUi>) -> Option<CardView> {
    let compact = view.compact;
    let data = &at.data;
    let id = format!("{}-{}", at.run.id.0, at.call_id);
    let ignore = |_: &str, _: &Value, _: &mut gpui::App| {};
    match at.tool.as_str() {
        "exec_command" | "write_stdin" => {
            #[cfg(feature = "terminal")]
            if let Some(term) = term::TermOutput::of(data, at.cut.is_some()) {
                let tree = term_card::body(&id, &term, at.cut.as_ref());
                let body = view.tree(
                    &format!("tools-term-{id}"),
                    &tree,
                    |action, args, cx| match action {
                        term_card::OPEN => {
                            if let Some(path) =
                                args.get("path").and_then(Value::as_str)
                            {
                                cx.open_with_system(std::path::Path::new(path));
                            }
                        }
                        term_card::COPY => {
                            if let Some(text) =
                                args.get("text").and_then(Value::as_str)
                            {
                                cx.write_to_clipboard(
                                    gpui::ClipboardItem::new_string(
                                        text.to_owned(),
                                    ),
                                );
                            }
                        }
                        _ => {}
                    },
                );
                return Some(CardView {
                    failed: term.failure(),
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
                body: (!lines.is_empty()).then(|| {
                    view.tree(
                        &format!("tools-out-{id}"),
                        &output(&lines),
                        ignore,
                    )
                }),
                ..CardView::default()
            })
        }
        "read" => {
            let read = ReadView::of(data)?;
            let key = read_key(at);
            let open = view
                .ui_value(&key)
                .and_then(Value::as_bool)
                .unwrap_or(false);
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
            let tree = read_body(&read, lang, open, missing.as_ref());
            let handle = view.handle.clone();
            let body = view.tree(
                &format!("tools-read-{id}"),
                &tree,
                move |action, args, cx| {
                    if action == MORE {
                        handle.set_ui(key.clone(), Some(args.clone()), cx);
                    }
                },
            );
            Some(CardView {
                label: Some(read.label()),
                body: Some(body),
                ..CardView::default()
            })
        }
        "edit" | "write" => {
            let result = data.result.as_ref().filter(|result| !result.error)?;
            let text = result.details.as_ref()?.get("diff")?.as_str()?;
            let tree = Node::Diff {
                text: text.to_owned(),
                path: data
                    .args
                    .get("path")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            };
            Some(CardView {
                label: Some(diff::stat(&diff::parse(text))),
                body: Some(view.tree(
                    &format!("tools-diff-{id}"),
                    &tree,
                    ignore,
                )),
                // A whole file can be long: a write's diff opens on a
                // click, an edit's few lines show at once.
                folds: at.tool == "write",
                ..CardView::default()
            })
        }
        listing::TOOL => {
            let result = data.result.as_ref().filter(|result| !result.error)?;
            let listing = DirListing::parse(result.details.as_ref()?)?;
            let head = listing_card::summary(&listing, &at.summary, compact);
            let body = listing_card::body(&listing, compact);
            Some(CardView {
                head: Some(view.tree(
                    &format!("tools-ls-head-{id}"),
                    &head,
                    ignore,
                )),
                label: Some(listing.summary()),
                body: Some(view.tree(&format!("tools-ls-{id}"), &body, ignore)),
                folds: true,
                ..CardView::default()
            })
        }
        _ => None,
    }
}
