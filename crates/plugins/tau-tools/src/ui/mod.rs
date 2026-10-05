//! tau-tools' UI (ADR 0017): the cards of its tools in a run's
//! transcript. `bash` shows its terminal, or the last lines of its
//! output; `edit` and `write`, the diff they made; `ls`, the directory.
//!
//! The host builds the plugin's tools on the run's workspace, where they
//! act (`tau-tools-host`); this UI draws what they return.

pub mod listing;
pub mod listing_card;
pub mod term;
#[cfg(feature = "terminal")]
pub mod term_card;

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use gpui::{Div, div, prelude::*, rems};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ui_kit::{
    diff,
    syntax,
    theme::{Design as _, MONO, Theme, Tone, Type, sp},
};
use tau_ui_plugin::{
    CallData,
    Fold,
    Manifest,
    PluginStatus,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRun, CardView},
};

use self::listing::DirListing;
use crate::artifact_grant::{ArtifactGrant, ArtifactMetadata, ArtifactRecord};

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
    previews: BTreeMap<String, Value>,
    /// The `read` cards opened past their first lines.
    reads: BTreeSet<(tau_agent::tool::RunId, String)>,
    /// Each `read` card's lines, highlighted once.
    colors: BTreeMap<(tau_agent::tool::RunId, String), Arc<syntax::Lines>>,
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

fn artifact_line(status: &ArtifactStatus, t: &Theme) -> Div {
    let label = match status {
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
    div()
        .px(sp(3.))
        .py(sp(1.))
        .child(tau_ui_kit::components::mono(label, Type::MICRO, t.muted))
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

/// A `read`'s first lines, numbered, and the row that shows the rest.
fn peek(
    read: &ReadView,
    colors: Option<&syntax::Lines>,
    open: bool,
    call_id: &str,
    toggle: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    t: &Theme,
) -> Div {
    let shown = if open {
        read.lines.len()
    } else {
        read.lines.len().min(PEEK)
    };
    let rest = read.lines.len() - read.lines.len().min(PEEK);
    let last = read.first + read.lines.len();
    // Half a rem per digit of the last line's number.
    let gutter = 0.5 * last.to_string().len() as f32;
    div()
        .flex()
        .flex_col()
        .pb(sp(1.5))
        .font_family(MONO)
        .typeset(Type::CAPTION)
        .line_height(rems(1.25))
        .children(read.lines[..shown].iter().enumerate().map(|(i, line)| {
            div()
                .flex()
                .gap(sp(3.))
                .px(sp(3.))
                .whitespace_nowrap()
                .overflow_hidden()
                .child(
                    div()
                        .flex_shrink_0()
                        .w(rems(gutter))
                        .flex()
                        .justify_end()
                        .text_color(t.dim.opacity(0.7))
                        .child((read.first + i).to_string()),
                )
                .child(
                    div().text_color(t.text_soft).child(
                        match colors.and_then(|colors| colors.get(i)) {
                            Some(parts) if !line.is_empty() => {
                                tau_ui_kit::select::selectable(
                                    gpui::StyledText::new(line.clone())
                                        .with_runs(syntax::runs(
                                            line,
                                            parts,
                                            t.text_soft,
                                            &t.syntax,
                                        )),
                                )
                                .into_any_element()
                            }
                            _ => tau_ui_kit::select::selectable(
                                gpui::StyledText::new(line.clone()),
                            )
                            .into_any_element(),
                        },
                    ),
                )
        }))
        .when(rest > 0, |body| {
            body.child(
                div()
                    .id(gpui::SharedString::from(format!("read-{call_id}")))
                    .flex()
                    .items_center()
                    .min_h(rems(1.75))
                    .px(sp(3.))
                    .pl(rems(gutter + 1.5))
                    .cursor_pointer()
                    .text_color(t.dim)
                    .hover(|row| row.text_color(t.text_soft))
                    .child(if open {
                        "show less".to_owned()
                    } else {
                        format!(
                            "+ {} more {}",
                            tau_ui_kit::format::grouped(rest),
                            if rest == 1 { "line" } else { "lines" }
                        )
                    })
                    .on_click(toggle),
            )
        })
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
        .line_height(rems(1.25))
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
                let t = view.theme().clone();
                let previews = view.ui.read(view.cx).previews.clone();
                let handle = view.handle.clone();
                Some(
                    div()
                        .flex()
                        .flex_col()
                        .gap(sp(2.))
                        .child(tau_ui_kit::components::heading(
                            &format!("Artifacts · {}", state.grants.len()),
                            &t,
                        ))
                        .when_some(state.error.clone(), |section, error| {
                            section.child(artifact_line(
                                &ArtifactStatus::Unavailable(error),
                                &t,
                            ))
                        })
                        .children(state.grants.values().map(move |grant| {
                            let id = grant.artifact.id.clone();
                            let status = ArtifactStatus::Available {
                                metadata: grant.artifact.clone(),
                                source: grant.source.clone(),
                                source_complete: grant.source_complete,
                            };
                            let preview = previews.get(&id).cloned();
                            let next_offset = preview
                                .as_ref()
                                .and_then(|value| value.get("next_offset"))
                                .and_then(Value::as_u64)
                                .unwrap_or(0);
                            let data = preview
                                .as_ref()
                                .and_then(|value| value.get("data"))
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let error = preview
                                .as_ref()
                                .and_then(|value| value.get("error"))
                                .and_then(Value::as_str);
                            let mut row = div()
                                .flex()
                                .flex_col()
                                .child(artifact_line(&status, &t));
                            if !data.is_empty() {
                                row = row.child(tau_ui_kit::components::mono(
                                    format!(
                                        "Preview at byte {}: {}",
                                        preview
                                            .as_ref()
                                            .and_then(|v| v.get("offset"))
                                            .and_then(Value::as_u64)
                                            .unwrap_or(0),
                                        data
                                    ),
                                    Type::CAPTION,
                                    t.text_soft,
                                ));
                            }
                            if let Some(error) = error {
                                row = row.child(artifact_line(
                                    &ArtifactStatus::Unavailable(
                                        error.to_owned(),
                                    ),
                                    &t,
                                ));
                            }
                            for (encoding, label) in [
                                ("utf8", "Preview text"),
                                ("base64", "Preview base64"),
                            ] {
                                let run = run.clone();
                                let id = id.clone();
                                let handle = handle.clone();
                                let action = json!(Action::Read {
                                    run,
                                    id: id.clone(),
                                    offset: next_offset,
                                    encoding: encoding.into()
                                });
                                row = row.child(
                                    div()
                                        .id(gpui::SharedString::from(format!(
                                            "artifact-{encoding}-{id}"
                                        )))
                                        .flex()
                                        .items_center()
                                        .min_h(rems(2.))
                                        .px(sp(3.))
                                        .py(sp(1.))
                                        .cursor_pointer()
                                        .child(tau_ui_kit::components::mono(
                                            label,
                                            Type::MICRO,
                                            t.blue,
                                        ))
                                        .on_click(move |_, _, cx| {
                                            handle.act(action.clone(), cx)
                                        }),
                                );
                            }
                            row
                        }))
                        .into_any_element(),
                )
            })
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
                Some(_) => tail(output_lines(data), 4),
                None => tail(output_lines(data), 6),
            };
            let artifact = artifact_error(data);
            Some(CardView {
                body: (!lines.is_empty() || artifact.is_some()).then(|| {
                    div()
                        .flex()
                        .flex_col()
                        .when(!lines.is_empty(), |body| {
                            body.child(output(&lines, &t))
                        })
                        .when_some(artifact.as_ref(), |body, status| {
                            body.child(artifact_line(status, &t))
                        })
                        .into_any_element()
                }),
                ..CardView::default()
            })
        }
        "read" => {
            let read = ReadView::of(data)?;
            let key = (at.run.id.clone(), at.call_id.clone());
            let open = view.ui.read(view.cx).reads.contains(&key);
            let colors = at
                .data
                .args
                .get("path")
                .and_then(Value::as_str)
                .and_then(syntax::Lang::of_path)
                .map(|lang| {
                    view.ui.update(view.cx, |ui, _| {
                        ui.colors
                            .entry(key.clone())
                            .or_insert_with(|| {
                                Arc::new(syntax::highlight_lines(
                                    lang,
                                    &read.lines,
                                ))
                            })
                            .clone()
                    })
                });
            let (ui, handle) = (view.ui.clone(), view.handle.clone());
            let toggle = move |_: &gpui::ClickEvent,
                               _: &mut gpui::Window,
                               cx: &mut gpui::App| {
                ui.update(cx, |ui, _| {
                    if !ui.reads.remove(&key) {
                        ui.reads.insert(key.clone());
                    }
                });
                handle.refresh(cx);
            };
            // Provenance is the inspector's; the card says only that
            // keeping it failed.
            let missing = artifact_error(data);
            Some(CardView {
                label: Some(read.label()),
                body: (!read.lines.is_empty() || missing.is_some()).then(
                    || {
                        div()
                            .flex()
                            .flex_col()
                            .when(!read.lines.is_empty(), |body| {
                                body.child(peek(
                                    &read,
                                    colors.as_deref(),
                                    open,
                                    &at.call_id,
                                    toggle,
                                    &t,
                                ))
                            })
                            .when_some(missing.as_ref(), |body, status| {
                                body.child(artifact_line(status, &t))
                            })
                            .into_any_element()
                    },
                ),
                ..CardView::default()
            })
        }
        "edit" | "write" => {
            let lines = diff_of(data)?;
            Some(CardView {
                label: Some(diff::stat(&lines)),
                body: Some(
                    diff::view(
                        &lines,
                        data.args
                            .get("path")
                            .and_then(Value::as_str)
                            .and_then(syntax::Lang::of_path),
                        &t,
                    )
                    .into_any_element(),
                ),
                // A whole file can be long: a write's diff opens on a
                // click, an edit's few lines show at once.
                folds: at.tool == "write",
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
            div()
                .flex()
                .flex_col()
                .child(term_card::body(
                    &ui,
                    &handle,
                    key,
                    &term,
                    at.cut.as_ref(),
                    t,
                    compact,
                    view.cx,
                ))
                .when_some(artifact_error(&at.data).as_ref(), |body, status| {
                    body.child(artifact_line(status, t))
                })
                .into_any_element(),
        ),
        inset: true,
        ..CardView::default()
    })
}
