//! tau-vcs's UI (ADR 0017): the cards of its tools in a run's
//! transcript (`vcs_status`, `vcs_diff`, `vcs_show`, `vcs_log`, and
//! `delegate` with what the sub-agent landed), and the pieces tau's
//! landings draw changes with.
//!
//! The host builds the plugin's tools with the run's workspace, which
//! they act on; this UI draws what they return.

pub mod change_diff;
pub mod change_log;
pub mod change_status;
pub mod diff_card;
pub mod landed;
pub mod log_card;
pub mod status_card;

use std::collections::{HashMap, HashSet};

use gpui::{
    AnyElement,
    App,
    ClickEvent,
    Entity,
    SharedString,
    Window,
    div,
    prelude::*,
};
use serde_json::Value;
use tau_agent::{plugin::Plugin, tool::RunId};
use tau_ui_kit::{components::link, theme::sp};
use tau_ui_plugin::{
    CallData,
    Handle,
    HostCx,
    Manifest,
    PluginInfo,
    RunCtx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, CardView},
};

use self::{
    change_diff::ChangeDiff,
    change_log::ChangeLog,
    change_status::ChangeStatus,
    landed::{LandedCard, LandingRecord},
};
use crate::details;

/// The name tau-vcs goes by in the interface.
pub const NAME: &str = "tau-vcs";

/// tau-vcs with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct VcsUi;

/// The cards' state in one window: the change picked in each log, and
/// the files open in each diff.
#[derive(Default)]
pub struct Ui {
    picked: HashMap<(RunId, String), String>,
    open_files: HashSet<(RunId, String, String)>,
}

impl Ui {
    /// Picks a change in a log card to show its detail, or puts it back
    /// when it is already the one picked.
    pub fn pick(&mut self, run: &RunId, call_id: &str, change_id: &str) {
        let key = (run.clone(), call_id.to_owned());
        if self.picked.get(&key).map(String::as_str) == Some(change_id) {
            self.picked.remove(&key);
        } else {
            self.picked.insert(key, change_id.to_owned());
        }
    }

    pub fn picked(&self, run: &RunId, call_id: &str) -> Option<&str> {
        self.picked
            .get(&(run.clone(), call_id.to_owned()))
            .map(String::as_str)
    }

    /// Opens a diff card's file to its hunks, or closes it.
    pub fn toggle_file(&mut self, run: &RunId, call_id: &str, path: &str) {
        let key = (run.clone(), call_id.to_owned(), path.to_owned());
        if !self.open_files.remove(&key) {
            self.open_files.insert(key);
        }
    }

    pub fn file_open(&self, run: &RunId, call_id: &str, path: &str) -> bool {
        self.open_files.contains(&(
            run.clone(),
            call_id.to_owned(),
            path.to_owned(),
        ))
    }
}

/// One card as its drawing reaches it: whose it is, the window's state
/// for it, and the way back.
pub struct Card<'a> {
    pub run: RunId,
    pub call_id: String,
    pub ui: &'a Ui,
    pub entity: Entity<Ui>,
    pub handle: Handle,
}

impl Card<'_> {
    /// A click that changes the window's state for cards, then draws
    /// again.
    pub fn on_ui(
        &self,
        f: impl Fn(&mut Ui) + 'static,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let (ui, handle) = (self.entity.clone(), self.handle.clone());
        move |_, _, cx| {
            ui.update(cx, |ui, _| f(ui));
            handle.refresh(cx);
        }
    }
}

/// What a tool returned, when it returned and did not fail.
fn details(data: &CallData) -> Option<&Value> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    result.details.as_ref()
}

/// A sub-agent's landing, from a `delegate` call.
pub fn delegated(data: &CallData) -> Option<LandedCard> {
    let details = details(data)?;
    Some(LandedCard::from_record(LandingRecord {
        from: details.get("run")?.as_str()?.to_owned(),
        title: data
            .args
            .get("task")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        landing: serde_json::from_value(details.get("landing")?.clone())
            .ok()?,
    }))
}

impl UiPlugin for VcsUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// None here: the host builds the tools with the run's workspace,
    /// which they act on.
    fn agent_plugins(
        &self,
        _host: &(),
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    fn catalog(&self, _host: &(), _cx: &HostCx, _settings: &()) -> PluginInfo {
        PluginInfo {
            description: "status diff log show describe commit new restore \
                          undo, on the run's workspace"
                .into(),
            seams: vec![Seam::Tools],
            page: None,
            ..Default::default()
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::CARD, card)
    }
}

/// A card of one of its tools.
fn card(at: &AtCard, view: &mut ViewCx<'_, VcsUi>) -> Option<CardView> {
    let t = view.theme().clone();
    let compact = view.compact;
    let ui = view.ui.clone();
    let state = ui.read(view.cx);
    let card = Card {
        run: at.run.id.clone(),
        call_id: at.call_id.clone(),
        ui: state,
        entity: ui.clone(),
        handle: view.handle.clone(),
    };
    let data = &at.data;
    match at.tool.as_str() {
        details::DELEGATE => {
            let landed = delegated(data)?;
            let child = landed.from.clone();
            let handle = view.handle.clone();
            Some(CardView {
                label: Some(match landed.changes.len() {
                    0 => "no changes".into(),
                    1 => "1 change landed".into(),
                    n => format!("{n} changes landed"),
                }),
                body: Some(
                    div()
                        .flex()
                        .flex_col()
                        .child(landed::landed_body(&landed, &t, compact))
                        .child(
                            div()
                                .id(SharedString::from(format!(
                                    "open-child-{}",
                                    child.0
                                )))
                                .px(sp(3.))
                                .py(sp(2.))
                                .border_t_1()
                                .border_color(t.border)
                                .child(link("Open the sub-agent's chat", &t))
                                .on_click(move |_, _, cx| {
                                    handle.open_run(&child, cx)
                                }),
                        )
                        .into_any_element(),
                ),
                ..CardView::default()
            })
        }
        change_status::TOOL => {
            let status = details(data).and_then(ChangeStatus::parse)?;
            Some(CardView {
                head: Some(
                    status_card::summary(&status, &t, compact)
                        .into_any_element(),
                ),
                label: Some(status.summary()),
                edge: status_card::edge(&status),
                body: Some(
                    status_card::body(&card, &status, &t, compact)
                        .into_any_element(),
                ),
                folds: true,
                ..CardView::default()
            })
        }
        change_diff::DIFF_TOOL | change_diff::SHOW_TOOL => {
            let diff = details(data).and_then(ChangeDiff::parse)?;
            let show = at.tool == change_diff::SHOW_TOOL;
            let (head, body): (AnyElement, AnyElement) = if show {
                (
                    diff_card::commit_summary(&diff, &t).into_any_element(),
                    diff_card::commit_body(&card, &diff, &t, compact)
                        .into_any_element(),
                )
            } else {
                (
                    diff_card::files_summary(&diff, &t).into_any_element(),
                    diff_card::files_body(&card, &diff, &t, compact)
                        .into_any_element(),
                )
            };
            Some(CardView {
                head: Some(head),
                label: Some(diff.stat()),
                shape: Some(diff_card::blocks(&diff, &t).into_any_element()),
                body: Some(body),
                folds: true,
                ..CardView::default()
            })
        }
        change_log::TOOL => {
            let log = details(data).and_then(ChangeLog::parse)?;
            Some(CardView {
                label: Some(log.summary()),
                shape: Some(log_card::bars(&log, &t).into_any_element()),
                body: Some(
                    log_card::body(&card, &log, &t, compact).into_any_element(),
                ),
                folds: true,
                ..CardView::default()
            })
        }
        _ => None,
    }
}
