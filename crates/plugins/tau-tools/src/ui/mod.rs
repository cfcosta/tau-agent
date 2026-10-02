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

use std::collections::BTreeMap;

use gpui::{Div, div, prelude::*, px};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::plugin::Plugin;
use tau_ui_kit::{
    diff,
    theme::{Design as _, MONO, Theme, Tone, Type, sp},
};
use tau_ui_plugin::{
    CallData,
    Fold,
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
#[cfg(feature = "host")]
use crate::artifact_grant::fold_grants;
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Read {
        run: tau_agent::tool::RunId,
        id: String,
        offset: u64,
        encoding: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct ActionReply {
    id: String,
    range: Option<Value>,
    error: Option<String>,
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
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
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
            description: "read, bash, edit, write, grep, find and ls on the \
                          run's workspace"
                .into(),
            seams: vec![Seam::Tools],
            page: None,
            ..Default::default()
        }
    }

    fn act(
        &self,
        _host: &(),
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        #[cfg(feature = "host")]
        {
            use anyhow::Context as _;
            use tau_artifacts::{Artifact, Bytes, Encoding, Quotas};
            let Action::Read {
                run,
                id,
                offset,
                encoding,
            } = serde_json::from_value(action)?;
            let read = (|| -> anyhow::Result<Value> {
                let encoding = match encoding.as_str() {
                    "utf8" => Encoding::Utf8,
                    "base64" => Encoding::Base64,
                    _ => anyhow::bail!("unsupported encoding"),
                };
                let starts = cx.runtime.block_on(
                    cx.store.plugin_entries(&run.0, tau_ui_plugin::HOST_RECORD),
                )?;
                let (_, start) =
                    starts.first().context("run has no repository")?;
                if starts.len() != 1 {
                    anyhow::bail!("ambiguous repository");
                }
                let start: tau_ui_plugin::HostRecord =
                    serde_json::from_str(start)?;
                let repo =
                    cx.repo(&start.repo).context("repository unavailable")?;
                let records: Vec<Value> = cx
                    .runtime
                    .block_on(cx.store.records(&run.0, NAME))?
                    .into_iter()
                    .map(|body| serde_json::from_str(&body))
                    .collect::<Result<_, _>>()?;
                let grants =
                    fold_grants(&records).map_err(anyhow::Error::msg)?;
                let grant = grants
                    .get(&id)
                    .context("artifact is not granted to this run")?;
                let artifact: Artifact = serde_json::from_value(
                    serde_json::to_value(&grant.artifact)?,
                )?;
                let bytes =
                    Bytes::new(repo.dir.join("artifacts"), Quotas::default())?;
                let range = bytes.read_range(
                    &artifact,
                    offset,
                    1024,
                    encoding,
                    &tokio_util::sync::CancellationToken::new(),
                )?;
                Ok(serde_json::to_value(range)?)
            })();
            Ok(Some(serde_json::to_value(match read {
                Ok(range) => ActionReply {
                    id,
                    range: Some(range),
                    error: None,
                },
                Err(error) => ActionReply {
                    id,
                    range: None,
                    error: Some(error.to_string()),
                },
            })?))
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (action, cx);
            Ok(None)
        }
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
                                        .min_h(px(32.))
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
            let artifact = artifact_status(data);
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
            let status = artifact_status(data)?;
            Some(CardView {
                body: Some(artifact_line(&status, &t).into_any_element()),
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
                .when_some(
                    artifact_status(&at.data).as_ref(),
                    |body, status| body.child(artifact_line(status, t)),
                )
                .into_any_element(),
        ),
        inset: true,
        ..CardView::default()
    })
}
