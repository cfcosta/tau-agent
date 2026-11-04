//! tau-vcs's UI (ADR 0017): the cards of its tools in a run's
//! transcript (`vcs_status`, `vcs_diff`, `vcs_show`, `vcs_log`, and
//! `spawn` with the way to the sub-agent's chat), and the pieces tau's
//! landings draw changes with.
//!
//! The host builds the plugin's tools with the run's workspace, which
//! they act on (`tau-vcs-host`); this UI draws what they return. Each
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

use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ui_plugin::{
    CallData,
    Manifest,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, CardView},
    tree::{Kind, Node, Tone},
    vcs::spawned,
};

use self::{
    change_diff::ChangeDiff,
    change_log::{Change, ChangeLog},
    change_status::ChangeStatus,
};
use crate::details;

/// The name tau-vcs goes by in the interface.
pub const NAME: &str = "tau-vcs";

/// tau-vcs with its UI.
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

/// A button that opens `child`'s chat.
pub fn open_child(child: &RunId) -> Node {
    Node::Button {
        label: "Open the sub-agent's chat".into(),
        action: OPEN_RUN.into(),
        args: json!({ "run": &*child.0 }),
        kind: Kind::Secondary,
        icon: Some("sub-agent".into()),
    }
}

/// A change's first line without its prefix, or that it has none.
pub(crate) fn subject(change: &Change) -> String {
    if change.subject.is_empty() {
        "(no description set)".into()
    } else {
        change.subject.clone()
    }
}

/// A conventional commit type's badge: `feat`, `fix`.
pub(crate) fn kind_badge(change: &Change) -> Option<Node> {
    let kind = change.kind.as_deref()?;
    let tone = match kind {
        "feat" => Tone::Info,
        "fix" => Tone::Bad,
        _ => Tone::Neutral,
    };
    Some(Node::Badge {
        text: kind.to_owned(),
        tone,
    })
}

/// A chip for each of the change's bookmarks, such as `main`.
pub(crate) fn bookmarks(change: &Change) -> impl Iterator<Item = Node> + '_ {
    change.info.bookmarks.iter().map(|name| Node::Pill {
        text: name.clone(),
        tone: Tone::Info,
    })
}

/// The change's short id.
pub(crate) fn short_id(change: &Change) -> Node {
    Node::Mono {
        text: change.short_id().to_owned(),
    }
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
                label: Some("started".into()),
                body: Some(open_child(&child)),
                ..CardView::default()
            })
        }
        details::COMMIT => {
            let commit = details(data).and_then(commit_card::Commit::parse)?;
            Some(CardView {
                head: Some(commit_card::summary(&commit)),
                label: Some(commit.label()),
                body: Some(commit_card::body(&card, &commit)),
                folds: true,
                ..CardView::default()
            })
        }
        change_status::TOOL => {
            let status = details(data).and_then(ChangeStatus::parse)?;
            Some(CardView {
                head: Some(status_card::summary(&status, card.compact)),
                label: Some(status.summary()),
                edge: status_card::edge(&status),
                body: Some(status_card::body(&card, &status)),
                folds: true,
                ..CardView::default()
            })
        }
        change_diff::DIFF_TOOL | change_diff::SHOW_TOOL => {
            let diff = details(data).and_then(ChangeDiff::parse)?;
            let (head, body) = if at.tool == change_diff::SHOW_TOOL {
                (
                    diff_card::commit_summary(&diff),
                    diff_card::commit_body(&card, &diff),
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
                body: Some(body),
                folds: true,
                ..CardView::default()
            })
        }
        change_log::TOOL => {
            let log = details(data).and_then(ChangeLog::parse)?;
            let body = log_card::body(&card, &log, picked.as_deref());
            Some(CardView {
                label: Some(log.summary()),
                body: Some(body),
                folds: true,
                ..CardView::default()
            })
        }
        _ => None,
    }
}
