//! heiko-vcs's UI (ADR 0017): the cards of its tools in a run's
//! transcript (`vcs_status`, `vcs_diff`, `vcs_show`, `vcs_log`,
//! `vcs_commit`, the tools that only confirm, each a line with its
//! result's word, and `spawn` with the way to the sub-agent's chat), and
//! the pieces heiko's landings draw changes with.
//!
//! The host builds the plugin's tools with the run's workspace, which
//! they act on (`heiko-vcs-host`); this UI draws what they return. Each
//! card is a view tree (ADR 0045): a diff card's files are sections the
//! interface opens and closes, and the change picked in a log is the
//! plugin's UI state, under [`pick_key`].

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
    points::{self, AtCard, CardView},
    tree::{Ink, Kind, Node, SquareLook, Tone},
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
/// null value clearing it. A log's rows pick their change with it.
const SET: &str = "set";
/// The action that opens a sub-agent's chat: `{"run": …}`.
const OPEN_RUN: &str = "open-run";

/// The UI state key holding the change picked in a log card, as its
/// full change id.
pub fn pick_key(run: &RunId, call_id: &str) -> String {
    format!("pick:{}:{call_id}", run.0)
}

/// The section id of a file in a card, whose open state is the
/// interface's (`tree::open_key`).
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

/// A diffstat's five squares: green by the share of lines added, red by
/// the share removed.
pub(crate) fn diffstat(added: usize, removed: usize) -> Node {
    use heiko_ui_kit::diff::{SQUARES, squares};
    let (green, red) = squares(added, removed);
    Node::row(
        [
            (green, Tone::Good),
            (red, Tone::Bad),
            (SQUARES - green - red, Tone::Neutral),
        ]
        .into_iter()
        .filter(|(count, _)| *count > 0)
        .map(|(count, tone)| Node::Toned {
            text: "■".repeat(count),
            tone,
            mono: true,
        }),
    )
}

/// The files a card lists before it says how many more there are.
pub(crate) const FILES_PEEK: usize = 12;

/// Each file on a row: its kind (`jj status`'s letter), its path, and
/// the lines it adds and removes; then how many files are past the
/// first [`FILES_PEEK`], and that the diff was cut, when it was.
pub(crate) fn file_rows(
    files: &[FileDiff],
    truncated: bool,
    conflicted: &dyn Fn(&str) -> bool,
) -> Vec<Node> {
    let toned = |text: String, tone| Node::Toned {
        text,
        tone,
        mono: true,
    };
    let mut rows: Vec<Node> = files
        .iter()
        .take(FILES_PEEK)
        .map(|file| {
            let tone = match file.kind {
                ChangeKind::Added => Tone::Good,
                ChangeKind::Modified => Tone::Info,
                ChangeKind::Removed => Tone::Bad,
            };
            let mut row = vec![
                toned(file.kind.letter().to_string(), tone),
                Node::Mono {
                    text: file.path.clone(),
                },
                Node::Spacer,
            ];
            if conflicted(&file.path) {
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

/// A badge for each of the change's states that wants a look:
/// `conflict`, `divergent`, `empty`.
pub(crate) fn flags(change: &Change) -> Vec<Node> {
    let info = &change.info;
    [
        (info.conflict, "conflict", Tone::Bad),
        (info.divergent, "divergent", Tone::Warn),
        (info.empty, "empty", Tone::Neutral),
    ]
    .into_iter()
    .filter(|(on, ..)| *on)
    .map(|(_, text, tone)| Node::Badge {
        text: text.into(),
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

    /// What a card's buttons ask: a log's pick, or a sub-agent's chat.
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

/// A card of one of its tools.
fn card(at: &AtCard, view: &mut ViewCx<'_, VcsUi>) -> Option<CardView> {
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
    match at.tool.as_str() {
        details::SPAWN => {
            let child = spawned(data)?;
            Some(CardView {
                shape: Some(Node::Stamp {
                    label: "started".into(),
                    tone: Ink::Good,
                }),
                body: Some(open_child(&child)),
                ..CardView::default()
            })
        }
        details::COMMIT => {
            let commit = details(data).and_then(commit_card::Commit::parse)?;
            Some(CardView {
                head: Some(commit_card::summary(&commit)),
                label: Some(commit.label()),
                body: Some(commit_card::body(&commit)),
                ..CardView::default()
            })
        }
        change_status::TOOL => {
            let status = details(data).and_then(ChangeStatus::parse)?;
            Some(CardView {
                head: Some(status_card::summary(&status)),
                label: Some(status.summary()),
                edge: status_card::edge(&status),
                body: status_card::body(&status),
                ..CardView::default()
            })
        }
        change_diff::DIFF_TOOL | change_diff::SHOW_TOOL => {
            let diff = details(data).and_then(ChangeDiff::parse)?;
            let (head, body) = if at.tool == change_diff::SHOW_TOOL {
                (
                    diff_card::commit_summary(&diff),
                    diff_card::commit_body(&diff),
                )
            } else {
                (
                    diff_card::files_summary(&diff),
                    diff_card::files_body(&card, &diff),
                )
            };
            Some(CardView {
                head: Some(head),
                label: Some(diff.stat()),
                shape: Some(diffstat(diff.added(), diff.removed())),
                body: Some(body),
                ..CardView::default()
            })
        }
        DESCRIBE | NEW => Some(CardView {
            label: Some(working_copy(details(data)?)?.short_id().to_owned()),
            ..CardView::default()
        }),
        RESTORE => {
            let restored = details(data)?.get("restored")?.as_array()?.len();
            Some(CardView {
                label: Some(match restored {
                    0 => "nothing to restore".into(),
                    n => format!("{} restored", count(n, "file")),
                }),
                ..CardView::default()
            })
        }
        RESOLVE => {
            let details = details(data)?;
            let resolved = details.get("resolved")?.as_array()?.len();
            let left = working_copy(details)?.info.conflict;
            Some(CardView {
                label: Some(if left {
                    format!("{resolved} resolved · conflicts left")
                } else {
                    "0 conflicts left".into()
                }),
                ..CardView::default()
            })
        }
        UNDO => {
            let tool = details(data)?.get("tool")?.as_str()?;
            Some(CardView {
                label: Some(format!("undid: {tool}")),
                ..CardView::default()
            })
        }
        LAND => {
            details(data)?
                .get("proposed")
                .and_then(Value::as_bool)
                .filter(|proposed| *proposed)?;
            Some(CardView {
                shape: Some(Node::Stamp {
                    label: "proposed".into(),
                    tone: Ink::Warn,
                }),
                ..CardView::default()
            })
        }
        change_log::TOOL => {
            let log = details(data).and_then(ChangeLog::parse)?;
            let body = log_card::body(&card, &log, picked.as_deref());
            Some(CardView {
                label: Some(log.summary()),
                body: Some(body),
                ..CardView::default()
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use heiko_ui_kit::diff::SQUARES;

    use super::*;

    /// A diffstat always has five squares, however the lines split.
    #[test]
    fn a_diffstat_has_five_squares() {
        for (added, removed) in [(0, 0), (3, 1), (38, 0), (0, 2), (1, 900)] {
            let Node::Row { children } = diffstat(added, removed) else {
                panic!("a row");
            };
            let squares: usize = children
                .iter()
                .map(|node| match node {
                    Node::Toned { text, .. } => text.chars().count(),
                    other => panic!("squares: {other:?}"),
                })
                .sum();
            assert_eq!(squares, SQUARES);
        }
    }
}
