//! tau-tools' UI (ADR 0017): the cards of its tools in a run's
//! transcript. `bash` shows its terminal, or the last lines of its
//! output; `edit` and `write`, the diff they made; `ls`, the directory.
//!
//! The host builds the plugin's tools on the run's workspace, where they
//! act; this UI draws what they return.

pub mod listing;
pub mod listing_card;
pub mod term;
#[cfg(feature = "terminal")]
pub mod term_card;

use gpui::{Context, Div, div, prelude::*, px};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_kit::{
    diff,
    theme::{Design as _, MONO, Theme, Tone, Type, sp},
};
use tau_ui_plugin::{
    CallData,
    Handle,
    HostCx,
    Manifest,
    PluginInfo,
    PluginStatus,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRun, CardView},
};

use self::listing::DirListing;

/// The name tau-tools goes by in the interface.
pub const NAME: &str = "tau-tools";

/// tau-tools with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolsUi;

/// The cards' state in one window: each command's screen.
#[derive(Default)]
pub struct Ui {
    #[cfg(feature = "terminal")]
    pub terms: term_card::TermCards,
}

/// The diff a call's result carries, when it carries one.
pub fn diff_of(data: &CallData) -> Option<Vec<diff::DiffLine>> {
    let result = data.result.as_ref().filter(|result| !result.error)?;
    let text = result.details.as_ref()?.get("diff")?.as_str()?;
    Some(diff::parse(text))
}

/// What a `bash` call printed, line by line: the text the model got
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

/// Output lines, a passing test's in green.
fn output(lines: &[String], t: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .px(sp(3.))
        .py(sp(2.))
        .font_family(MONO)
        .typeset(Type::CAPTION)
        .line_height(px(20.))
        .text_color(t.muted)
        .children(lines.iter().map(|line| {
            let pass = line.trim_start().starts_with("PASS");
            div()
                .whitespace_nowrap()
                .overflow_hidden()
                .when(pass, |row| row.text_color(t.green))
                .child(line.clone())
        }))
}

impl UiPlugin for ToolsUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    fn host(&self, _cx: &HostCx) -> anyhow::Result<()> {
        Ok(())
    }

    /// None here: the host builds the tools on the run's workspace,
    /// where they act.
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
            name: NAME.into(),
            description: "read, bash, edit, write, grep, find and ls on the \
                          run's workspace"
                .into(),
            seams: vec![Seam::Tools],
            spend: 0.0,
            page: None,
        }
    }

    fn apply(&self, _state: &mut (), _body: &Value, _run: &mut dyn RunCx) {}

    fn new_ui(&self, _handle: Handle, _cx: &mut Context<Ui>) -> Ui {
        Ui::default()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::CARD, card)
            .contribute_at(points::STATUS, -20, |_: &AtRun, _| {
                Some(PluginStatus {
                    name: NAME.into(),
                    state: "7 tools".into(),
                    tone: Tone::Quiet,
                })
            })
    }
}

/// A card of one of its tools.
fn card(at: &AtCard, view: &mut ViewCx<'_, ToolsUi>) -> Option<CardView> {
    let t = view.theme().clone();
    let compact = view.compact;
    let data = &at.data;
    match at.tool.as_str() {
        "bash" => {
            #[cfg(feature = "terminal")]
            if let Some(view) = terminal(at, view, &t, compact) {
                return Some(view);
            }
            let lines = match &data.result {
                Some(result) if result.error => return None,
                Some(_) => tail(output_lines(data), 4),
                None => tail(output_lines(data), 6),
            };
            Some(CardView {
                body: (!lines.is_empty())
                    .then(|| output(&lines, &t).into_any_element()),
                ..CardView::default()
            })
        }
        "edit" | "write" => {
            let lines = diff_of(data)?;
            Some(CardView {
                label: Some(diff::stat(&lines)),
                body: Some(diff::view(&lines, &t).into_any_element()),
                ..CardView::default()
            })
        }
        listing::TOOL => {
            let result = data.result.as_ref().filter(|result| !result.error)?;
            let listing = DirListing::parse(result.details.as_ref()?)?;
            Some(CardView {
                head: Some(
                    listing_card::summary(&listing, &at.summary, &t, compact)
                        .into_any_element(),
                ),
                label: Some(listing.summary()),
                body: Some(
                    listing_card::body(&listing, &t, compact)
                        .into_any_element(),
                ),
                folds: true,
                ..CardView::default()
            })
        }
        _ => None,
    }
}

/// A command run under a terminal: its screen.
#[cfg(feature = "terminal")]
fn terminal(
    at: &AtCard,
    view: &mut ViewCx<'_, ToolsUi>,
    t: &Theme,
    compact: bool,
) -> Option<CardView> {
    let key = (at.run.id.clone(), at.call_id.clone());
    let cut = at.cut.is_some();
    let term = view
        .ui
        .update(view.cx, |ui, _| ui.terms.output(&key, &at.data, cut))?;
    let ui = view.ui.clone();
    let handle = view.handle.clone();
    Some(CardView {
        failed: term.failure(),
        body: Some(
            term_card::body(
                &ui,
                &handle,
                key,
                &term,
                at.cut.as_ref(),
                t,
                compact,
                view.cx,
            )
            .into_any_element(),
        ),
        inset: true,
        ..CardView::default()
    })
}
