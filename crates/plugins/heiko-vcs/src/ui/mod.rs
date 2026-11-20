//! heiko-vcs's UI (ADR 0017): the cards of its tools in a run's
//! transcript (`vcs_status`, `vcs_diff`, `vcs_show`, `vcs_log`,
//! `vcs_commit`, the tools that only confirm, each a line with its
//! result's word, and `spawn` with the way to the sub-agent's chat), and
//! the pieces heiko's landings draw changes with.
//!
//! The host builds the plugin's tools with the run's workspace, which
//! they act on (`heiko-vcs-host`); this UI draws what they return. Each
//! card is a view tree (ADR 0045): which of a diff card's files show
//! their hunks and the change picked in a log are the plugin's UI state,
//! under [`Card::file_key`] and [`pick_key`].

pub mod change_diff;
pub mod change_log;
pub mod change_status;
pub mod commit_card;
pub mod diff_card;
pub mod landed;
pub mod log_card;
pub mod status_card;

use heiko_agent::tool::RunId;
use heiko_ui_plugin::{
    CallData,
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, AtCard},
    step::{Body, Receipt, Step},
    tree::{Ink, Kind, Node, SquareLook, Tone, open_key},
    vcs::{LAND, spawned},
};
use serde_json::{Value, json};

use self::{
    change_diff::{ChangeDiff, FileDiff},
    change_log::{Change, ChangeLog},
    change_status::ChangeStatus,
};
use crate::{ChangeInfo, ChangeKind, details};

/// The names the model calls the tools that only confirm by.
const DESCRIBE: &str = "vcs_describe";
const NEW: &str = "vcs_new";
const RESTORE: &str = "vcs_restore";
const RESOLVE: &str = "vcs_resolve";
const UNDO: &str = "vcs_undo";

/// The name heiko-vcs goes by in the interface.
pub const NAME: &str = "heiko-vcs";

/// heiko-vcs with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct VcsUi;

/// The action that sets a UI state key: `{"key": …, "value": …}`, a
/// null value clearing it. A log's rows pick their change with it, and
/// a diff's files open with it.
const SET: &str = "set";
/// The action that opens a sub-agent's chat: `{"run": …}`.
const OPEN_RUN: &str = "open-run";

/// The UI state key holding the change picked in a log card, as its
/// full change id.
pub fn pick_key(run: &RunId, call_id: &str) -> String {
    format!("pick:{}:{call_id}", run.0)
}

/// The id of a file in a card; whether its hunks show is the UI state
/// under this id's `tree::open_key`.
pub fn file_id(run: &RunId, call_id: &str, path: &str) -> String {
    format!("file:{}:{call_id}:{path}", run.0)
}

/// What a log's pick becomes when `change_id` is clicked: that change,
/// or none when it is already the one picked.
pub fn pick(current: Option<&str>, change_id: &str) -> Option<String> {
    (current != Some(change_id)).then(|| change_id.to_owned())
}

/// One card as its tree is built: whose it is, and whether it is drawn
/// on a phone.
#[derive(Debug, Clone)]
pub struct Card {
    pub run: RunId,
    pub call_id: String,
    pub compact: bool,
}

impl Card {
    pub fn file_id(&self, path: &str) -> String {
        file_id(&self.run, &self.call_id, path)
    }

    /// The UI state key holding whether `path`'s hunks show.
    pub fn file_key(&self, path: &str) -> String {
        open_key(&self.file_id(path))
    }

    pub fn pick_key(&self) -> String {
        pick_key(&self.run, &self.call_id)
    }
}

/// What a tool returned, when it returned and did not fail.
fn details(data: &CallData) -> Option<&Value> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    result.details.as_ref()
}

/// The working copy a confirming tool's result names.
fn working_copy(details: &Value) -> Option<Change> {
    let info: ChangeInfo =
        serde_json::from_value(details.get("working_copy")?.clone()).ok()?;
    Some(Change::new(info))
}

/// A button that opens `child`'s chat.
pub fn open_child(child: &RunId) -> Node {
    Node::Button {
        label: "Open the sub-agent's chat".into(),
        action: OPEN_RUN.into(),
        args: json!({ "run": &*child.0 }),
        kind: Kind::Secondary,
        icon: Some("sub-agent".into()),
        size: Default::default(),
        wide: false,
        detail: None,
    }
}

/// A change's first line, or that it has none.
pub(crate) fn first_line(change: &Change) -> String {
    match change.info.description.lines().next() {
        Some(line) if !line.trim().is_empty() => line.trim().to_owned(),
        _ => "(no description set)".into(),
    }
}

/// A change's place on the line of changes: a square, filled for the
/// one the card is about.
pub(crate) fn node(on: bool) -> Node {
    Node::Square {
        label: String::new(),
        style: if on {
            SquareLook::Filled
        } else {
            SquareLook::Quiet
        },
        tone: if on { Ink::Purple } else { Ink::Neutral },
        action: None,
        args: Default::default(),
    }
}

/// Each of the change's bookmarks, such as `main`.
pub(crate) fn bookmarks(change: &Change) -> impl Iterator<Item = Node> + '_ {
    change.info.bookmarks.iter().map(|name| Node::Toned {
        text: name.clone(),
        tone: Tone::Good,
        mono: true,
    })
}

/// The change's short id.
pub(crate) fn short_id(change: &Change) -> Node {
    Node::Mono {
        text: change.short_id().to_owned(),
    }
}

/// A line that matters less than the ones around it.
pub(crate) fn quiet(text: impl Into<String>) -> Node {
    Node::Toned {
        text: text.into(),
        tone: Tone::Neutral,
        mono: true,
    }
}

/// The files a card lists before it says how many more there are.
pub(crate) const FILES_PEEK: usize = 12;

/// A file's row: its kind (`jj status`'s letter), `name` (its path),
/// and at the end that it holds a conflict, when `conflicted`, and the
/// lines it adds and removes.
pub(crate) fn file_row(file: &FileDiff, name: Node, conflicted: bool) -> Node {
    let toned = |text: String, tone| Node::Toned {
        text,
        tone,
        mono: true,
    };
    let tone = match file.kind {
        ChangeKind::Added => Tone::Good,
        ChangeKind::Modified => Tone::Info,
        ChangeKind::Removed => Tone::Bad,
    };
    let mut row = vec![
        toned(file.kind.letter().to_string(), tone),
        name,
        Node::Spacer,
    ];
    if conflicted {
        row.push(Node::Stamp {
            label: "conflict".into(),
            tone: Ink::Bad,
        });
    }
    if file.binary {
        row.push(quiet("binary"));
    } else {
        row.push(toned(format!("+{}", file.added), Tone::Good));
        row.push(toned(format!("−{}", file.removed), Tone::Bad));
    }
    Node::row(row)
}

/// Each file on a [`file_row`]; then how many files are past the first
/// [`FILES_PEEK`], and that the diff was cut, when it was.
pub(crate) fn file_rows(
    files: &[FileDiff],
    truncated: bool,
    conflicted: &dyn Fn(&str) -> bool,
) -> Vec<Node> {
    let mut rows: Vec<Node> = files
        .iter()
        .take(FILES_PEEK)
        .map(|file| {
            let path = Node::Mono {
                text: file.path.clone(),
            };
            file_row(file, path, conflicted(&file.path))
        })
        .collect();
    let more = files.len().saturating_sub(FILES_PEEK);
    if more > 0 {
        rows.push(quiet(format!("+ {}", count(more, "more file"))));
    }
    if truncated {
        rows.push(quiet("cut at 50 KB: files past the cut are not listed"));
    }
    rows
}

/// A stamp for each of the change's states that wants a look:
/// `conflict`, `divergent`, `empty`.
pub(crate) fn flags(change: &Change) -> Vec<Node> {
    let info = &change.info;
    [
        (info.conflict, "conflict", Ink::Bad),
        (info.divergent, "divergent", Ink::Warn),
        (info.empty, "empty", Ink::Neutral),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, label, tone)| Node::Stamp {
        label: label.into(),
        tone,
    })
    .collect()
}

/// `n noun`, or `n nouns`.
pub(crate) fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

impl UiPlugin for VcsUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> heiko_ui_plugin::Declaration {
        heiko_ui_plugin::Declaration::new()
            .fills(&[heiko_ui_plugin::vcs::VCS.name()])
            .seams(&[
                heiko_ui_plugin::Seam::Tools,
                heiko_ui_plugin::Seam::Start,
                heiko_ui_plugin::Seam::AfterTool,
                heiko_ui_plugin::Seam::OnEvent,
                heiko_ui_plugin::Seam::BeforeStop,
                heiko_ui_plugin::Seam::Finish,
            ])
            .services(&[heiko_ui_plugin::services::names::IDENTITY])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::CARD, card)
            .contribute(points::CHANGES, landed::changes)
    }

    /// What a card's buttons ask: a log's pick, a diff's file opened,
    /// or a sub-agent's chat.
    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        match action {
            SET => {
                let Some(key) = args.get("key").and_then(Value::as_str) else {
                    return;
                };
                let value = args.get("value").filter(|value| !value.is_null());
                view.handle.set_ui(key, value.cloned());
            }
            OPEN_RUN => {
                if let Some(run) = args.get("run").and_then(Value::as_str) {
                    view.handle.open_run(&RunId(run.into()));
                }
            }
            _ => {}
        }
    }
}

/// A body the person works in: picks, opens and links, drawn here.
fn tree(node: Node) -> Option<Body> {
    Some(Body::Tree {
        node,
        folds: false,
        inset: false,
    })
}

/// A call to one of its tools, as a step.
fn card(at: &AtCard, view: &mut ViewCx<'_, VcsUi>) -> Option<Step> {
    let card = Card {
        run: at.run.id.clone(),
        call_id: at.call_id.clone(),
        compact: view.compact,
    };
    let data = &at.data;
    let picked = view
        .ui_value(&card.pick_key())
        .and_then(Value::as_str)
        .map(str::to_owned);
    let said = |text: String| vec![Receipt::measure(text)];
    match at.tool.as_str() {
        details::SPAWN => {
            let child = spawned(data)?;
            Some(Step {
                receipt: vec![Receipt::stamp("started", Ink::Good)],
                body: tree(open_child(&child)),
                ..Step::default()
            })
        }
        details::COMMIT => {
            let commit = details(data).and_then(commit_card::Commit::parse)?;
            Some(Step {
                subject: Some(commit_card::summary(&commit)),
                receipt: said(commit.label()),
                body: tree(commit_card::body(&commit)),
                ..Step::default()
            })
        }
        change_status::TOOL => {
            let status = details(data).and_then(ChangeStatus::parse)?;
            Some(Step {
                subject: Some(status_card::summary(&status)),
                receipt: status_card::warning(&status)
                    .into_iter()
                    .chain([Receipt::measure(status.summary())])
                    .collect(),
                body: status_card::body(&status).and_then(tree),
                ..Step::default()
            })
        }
        change_diff::DIFF_TOOL | change_diff::SHOW_TOOL => {
            let diff = details(data).and_then(ChangeDiff::parse)?;
            let (subject, body) = if at.tool == change_diff::SHOW_TOOL {
                (
                    diff_card::commit_summary(&diff),
                    diff_card::commit_body(&diff),
                )
            } else {
                let open = |path: &str| {
                    view.ui_value(&card.file_key(path))
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                };
                (
                    diff_card::files_summary(&diff),
                    diff_card::files_body(&card, &diff, &open),
                )
            };
            Some(Step {
                subject: Some(subject),
                receipt: vec![Receipt::Diff {
                    added: diff.added() as u64,
                    removed: diff.removed() as u64,
                    new: false,
                }],
                body: tree(body),
                ..Step::default()
            })
        }
        DESCRIBE | NEW => Some(Step {
            receipt: said(working_copy(details(data)?)?.short_id().to_owned()),
            ..Step::default()
        }),
        RESTORE => {
            let restored = details(data)?.get("restored")?.as_array()?.len();
            Some(Step {
                receipt: vec![match restored {
                    0 => Receipt::measure("nothing to restore"),
                    n => Receipt::measure(format!(
                        "{} restored",
                        heiko_ui_plugin::step::counted(n, "file")
                    )),
                }],
                ..Step::default()
            })
        }
        RESOLVE => {
            let details = details(data)?;
            let resolved = details.get("resolved")?.as_array()?.len();
            let left = working_copy(details)?.info.conflict;
            Some(Step {
                receipt: said(if left {
                    format!("{resolved} resolved · conflicts left")
                } else {
                    "0 conflicts left".into()
                }),
                ..Step::default()
            })
        }
        UNDO => {
            let tool = details(data)?.get("tool")?.as_str()?;
            Some(Step {
                receipt: said(format!("undid: {tool}")),
                ..Step::default()
            })
        }
        LAND => {
            details(data)?
                .get("proposed")
                .and_then(Value::as_bool)
                .filter(|proposed| *proposed)?;
            Some(Step {
                receipt: vec![Receipt::stamp("proposed", Ink::Warn)],
                ..Step::default()
            })
        }
        change_log::TOOL => {
            let log = details(data).and_then(ChangeLog::parse)?;
            let body = log_card::body(&card, &log, picked.as_deref());
            Some(Step {
                receipt: said(log.summary()),
                body: tree(body),
                ..Step::default()
            })
        }
        _ => None,
    }
}
