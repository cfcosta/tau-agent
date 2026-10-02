//! tau-codemode's UI (ADR 0017): the `codemode` card, with the script,
//! the calls it made, its output and what it cost; the store a run's
//! scripts keep, in the inspector; and its line in the run's plugin
//! list.
//!
//! A card's calls come from one of two places, and agree:
//!
//! - while the script runs, from the nested calls tau-ui folds into the
//!   card's [`CallData::nested`] from run events, and from the Jev rows
//!   the call reports in its updates ([`CallData::updates`]);
//! - once it ends, from its result's `details.calls`, the only record a
//!   stored run has. tau-ui empties the live rows then, so a live card
//!   and a stored one draw the same thing by construction.
//!
//! Plugins' verdicts on a nested call ([`CallData::nested_marks`]) mark
//! its row in both.

use std::{collections::BTreeMap, sync::Arc};

use gpui::{Div, div, prelude::*, px};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, code_block, heading, icon, mono},
    format::fine_usd,
    theme::{IconSize, Theme, Tone, Type, sp},
};
use tau_ui_plugin::{
    CallData,
    CardMark,
    Fold,
    HostCx,
    Manifest,
    NestedMark,
    PluginInfo,
    PluginStatus,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRun, CardView},
};

use crate::{
    CallStatus,
    PLUGIN,
    description::NAME,
    live::JevUpdate,
    modules,
    result::{MAX_ARGS_CHARS, MAX_ERROR_CHARS, preview},
    store::{Record, Snapshot, Writes},
};

/// tau-codemode with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodemodeUi;

/// What a run's scripts kept in the store, as its records leave it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The store's values, by key.
    pub store: Snapshot,
    /// The writes folded: one per script that wrote something.
    pub writes: usize,
    /// Immutable module versions and the current version of each name.
    pub modules: modules::Library,
    /// Private inference records; a start without a finish is interrupted.
    pub inference: Vec<crate::inference_trace::Record>,
}

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's records: a script's store writes.
    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        match record {
            Record::Store(writes) => {
                writes.apply(&mut self.store);
                self.writes += 1;
            }
            Record::Module(record) => {
                let _ = self.modules.apply(&record);
            }
            Record::Inference(record) => self.inference.push(record),
        }
    }
}

impl State {
    /// A start stays incomplete until a matching durable finish arrives.
    pub fn inference_statuses(&self) -> BTreeMap<String, &'static str> {
        let mut statuses = BTreeMap::new();
        for record in &self.inference {
            match record {
                crate::inference_trace::Record::Started {
                    trace_id, ..
                }
                | crate::inference_trace::Record::Attempt {
                    trace_id, ..
                } => {
                    statuses
                        .entry(trace_id.clone())
                        .or_insert("interrupted / incomplete");
                }
                crate::inference_trace::Record::Finished {
                    trace_id,
                    complete,
                    ..
                } => {
                    statuses.insert(
                        trace_id.clone(),
                        if *complete {
                            "finished"
                        } else {
                            "interrupted / incomplete"
                        },
                    );
                }
            }
        }
        statuses
    }
}

/// One call a script made, as its card lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    /// The tool, or `jev.<function>`.
    pub name: String,
    /// Compact JSON, cut at [`MAX_ARGS_CHARS`].
    pub args: String,
    pub status: CallStatus,
    /// How long it took; known once the script ended.
    pub ms: Option<u64>,
    /// Cut at [`MAX_ERROR_CHARS`].
    pub error: Option<String>,
    /// US dollars, for calls that cost something (Jev).
    pub cost: Option<f64>,
    /// What plugins decided about it.
    pub marks: Vec<NestedMark>,
}

/// A card's calls, and whether they are all there.
#[derive(Debug, Clone, PartialEq)]
pub struct Calls {
    pub rows: Vec<Row>,
    /// False past the result's cap on rows.
    pub complete: bool,
}

/// The calls the script `call_id` has made so far, in the order they
/// started, as its result's `details.calls` will list them:
///
/// - its tool calls, from the nested calls tau-ui folds into its card.
///   Only its own: a call one of them made is that call's business;
/// - its Jev requests, which make no run events, from the updates the
///   call reported ([`JevUpdate`]), the latest of each standing. Each
///   goes after the tool calls started before it.
///
/// The order events and updates came in does not matter: rows are
/// placed by their ids and by what the updates say.
pub fn live_rows(call_id: &str, data: &CallData) -> Vec<Row> {
    let prefix = format!("{call_id}/");
    // (tool calls before it, 0 for a tool call and 1 for Jev, its
    // number), with the number of a tool call its own place.
    let mut keyed: Vec<((usize, u8, usize), Row)> = data
        .nested
        .iter()
        .filter(|call| call.parent == call_id)
        .map(|call| {
            let (status, error) = match &call.result {
                None => (CallStatus::Running, None),
                Some(result) if result.error => (
                    CallStatus::Error,
                    Some(preview(&result.text, MAX_ERROR_CHARS)),
                ),
                Some(_) => (CallStatus::Ok, None),
            };
            let n = call
                .id
                .strip_prefix(&prefix)
                .and_then(|n| n.parse().ok())
                .unwrap_or(usize::MAX);
            let row = Row {
                id: call.id.clone(),
                name: call.tool.clone(),
                args: preview(&call.args.to_string(), MAX_ARGS_CHARS),
                status,
                ms: None,
                error,
                cost: None,
                marks: Vec::new(),
            };
            ((n, 0, 0), row)
        })
        .collect();
    let mut jev: Vec<((usize, u8, usize), Row)> = Vec::new();
    for update in data.updates.iter().filter_map(JevUpdate::from_details) {
        let Some(mut row) = row_of(&update.row) else {
            continue;
        };
        if row.status == CallStatus::Running {
            row.ms = None;
        }
        let n = row
            .id
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_prefix("jev/"))
            .and_then(|n| n.parse().ok())
            .unwrap_or(usize::MAX);
        match jev.iter_mut().find(|(_, kept)| kept.id == row.id) {
            Some(kept) => *kept = ((update.after, 1, n), row),
            None => jev.push(((update.after, 1, n), row)),
        }
    }
    keyed.extend(jev);
    // Stable: calls whose ids say nothing keep the order they came in.
    keyed.sort_by_key(|(key, _)| *key);
    keyed.into_iter().map(|(_, row)| row).collect()
}

/// A row of `details.calls`.
fn row_of(row: &Value) -> Option<Row> {
    Some(Row {
        id: row.get("id")?.as_str()?.to_owned(),
        name: row.get("name")?.as_str()?.to_owned(),
        args: row
            .get("args")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        status: CallStatus::parse(row.get("status")?.as_str()?)?,
        ms: row.get("ms").and_then(Value::as_u64),
        error: row.get("error").and_then(Value::as_str).map(str::to_owned),
        cost: row.get("cost").and_then(Value::as_f64),
        marks: Vec::new(),
    })
}

/// The calls a finished script's `details` list, or `None` when they
/// list none (a call that failed before the script ran).
pub fn stored_rows(details: &Value) -> Option<Calls> {
    let rows = details
        .get("calls")?
        .as_array()?
        .iter()
        .filter_map(row_of)
        .collect();
    Some(Calls {
        rows,
        complete: details
            .get("complete")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    })
}

/// The calls the card `call_id` lists: its result's once it ended, else
/// those made so far; each with what plugins decided about it.
pub fn calls(call_id: &str, data: &CallData) -> Calls {
    let mut calls = data
        .result
        .as_ref()
        .and_then(|result| result.details.as_ref())
        .and_then(stored_rows)
        .unwrap_or_else(|| Calls {
            rows: live_rows(call_id, data),
            complete: true,
        });
    for row in &mut calls.rows {
        row.marks = data.marks_of(&row.id).cloned().collect();
    }
    calls
}

/// What a finished script said: its output, and why it failed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Said {
    pub output: String,
    pub error: Option<String>,
}

/// The output and failure in a result's text: what follows its
/// `Output:` line, the failure's `Script error:` item apart. A call
/// that failed before the script ran (a bad options line) has only its
/// message.
pub fn said(text: &str, failed: bool) -> Said {
    let Some((_, rest)) = text.split_once("\nOutput:\n") else {
        return Said {
            output: String::new(),
            error: failed.then(|| text.trim().to_owned()),
        };
    };
    // tau-ui joins the result's text blocks with a newline.
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let (output, error) = match rest.rsplit_once("Script error:\n") {
        Some((output, error)) if failed => {
            // The list of calls made before it is the card's rows.
            let head = error.split("\n\n").next().unwrap_or(error);
            (output, Some(head.trim().to_owned()))
        }
        _ => (rest, None),
    };
    Said {
        output: output.trim_end().to_owned(),
        error,
    }
}

/// The script's source, from the call's arguments.
pub fn script(data: &CallData) -> &str {
    data.args
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The script's first line worth reading: not its options line.
pub fn first_line(code: &str) -> &str {
    code.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("-- @options:"))
        .unwrap_or_default()
}

/// What Jev cost a finished script, from its details' `usage`.
pub fn cost(details: &Value) -> f64 {
    details["usage"]["cost"]["total"].as_f64().unwrap_or(0.0)
}

/// The header's line: `3 calls · 1.2 s · $0.0004`. While the script
/// runs (no details yet), the calls so far and what its Jev requests
/// have cost so far.
pub fn label(calls: &Calls, details: Option<&Value>) -> String {
    let n = calls.rows.len();
    let mut parts = vec![match (n, calls.complete) {
        (1, _) => "1 call".to_owned(),
        (n, true) => format!("{n} calls"),
        (n, false) => format!("{n}+ calls"),
    }];
    if let Some(details) = details {
        if let Some(ms) = details["wall_ms"].as_u64() {
            parts.push(format!("{:.1} s", ms as f64 / 1000.0));
        }
        let cost = cost(details);
        if cost > 0.0 {
            parts.push(fine_usd(cost));
        }
    } else {
        let cost: f64 = calls.rows.iter().filter_map(|row| row.cost).sum();
        if cost > 0.0 {
            parts.push(fine_usd(cost));
        }
    }
    parts.join(" · ")
}

/// The most lines of output a card shows.
const OUTPUT_LINES: usize = 40;

impl UiPlugin for CodemodeUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        PLUGIN
    }

    /// The `codemode` tool, asking the run's metered Jev when there is a
    /// key; without one, scripts run and `jev` is nil.
    fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        #[cfg(feature = "host")]
        {
            let jev = run.services.get::<Arc<dyn Jev>>().cloned();
            Ok(vec![Box::new(crate::Codemode::new(jev))])
        }
        // Without its host half, a run gets no `codemode` tool.
        #[cfg(not(feature = "host"))]
        {
            let _ = run;
            Ok(Vec::new())
        }
    }

    fn catalog(&self, _host: &(), cx: &HostCx, _settings: &()) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        PluginInfo {
            description: description(jev),
            seams: vec![Seam::Start, Seam::Tools],
            page: None,
            ..Default::default()
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::CARD, card)
            .contribute(points::INSPECTOR, |_: &AtRun, view| {
                let state = view.state?;
                let t = view.theme().clone();
                Some(store_section(state, &t).into_any_element())
            })
            .contribute(points::STATUS, |at: &AtRun, view| {
                let scripts = view
                    .cards(&at.run.id)
                    .iter()
                    .filter(|card| card.tool == NAME)
                    .count();
                let keys = view.state.map_or(0, |state| state.store.len());
                let state = match (scripts, keys) {
                    (0, 0) => "no scripts yet".to_owned(),
                    (1, keys) => format!("1 script · {keys} stored"),
                    (n, keys) => format!("{n} scripts · {keys} stored"),
                };
                Some(PluginStatus {
                    name: PLUGIN.into(),
                    state,
                    tone: Tone::Quiet,
                })
            })
    }
}

/// Its entry on the Plugins screen: Jev is optional.
pub fn description(jev: bool) -> String {
    if jev {
        "Runs Luau scripts that call tools, with Jev".to_owned()
    } else {
        "Runs Luau scripts that call tools; with a TypeSafe key (Models), \
         scripts ask Jev too"
            .to_owned()
    }
}

/// The `codemode` card.
pub fn card(
    at: &AtCard,
    view: &mut ViewCx<'_, CodemodeUi>,
) -> Option<CardView> {
    if at.tool != NAME {
        return None;
    }
    let t = view.theme().clone();
    let data = &at.data;
    let calls = calls(&at.call_id, data);
    let details = data.result.as_ref().and_then(|r| r.details.as_ref());
    let said = data
        .result
        .as_ref()
        .map(|result| said(&result.text, result.error));
    let failed = said.as_ref().and_then(|said| said.error.clone());
    let code = script(data);
    Some(CardView {
        head: Some(
            mono(first_line(code).to_owned(), Type::CAPTION, t.text_soft)
                .flex_1()
                .min_w(px(0.))
                .truncate()
                .into_any_element(),
        ),
        label: Some(label(&calls, details)),
        failed: failed
            .as_ref()
            .map(|error| error.lines().next().unwrap_or_default().to_owned()),
        edge: failed.as_ref().map(|_| Tone::Danger),
        shape: None,
        body: Some(
            body(code, &calls, said.as_ref(), details, &t).into_any_element(),
        ),
        // Open while it runs, so its calls show as they come; closed once
        // it ended, down to its header.
        folds: data.result.is_some(),
        inset: false,
    })
}

/// The script, its calls, its output and what it stored.
fn body(
    code: &str,
    calls: &Calls,
    said: Option<&Said>,
    details: Option<&Value>,
    t: &Theme,
) -> Div {
    let writes = details
        .and_then(|details| details.get("store"))
        .and_then(|store| serde_json::from_value::<Writes>(store.clone()).ok())
        .filter(|writes| !writes.is_empty());
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .px(sp(3.))
        .py(sp(2.5))
        .child(code_block(Some("luau"), code, t))
        .when(!calls.rows.is_empty(), |body| {
            body.child(
                div()
                    .flex()
                    .flex_col()
                    .child(heading(&format!("Calls · {}", calls.rows.len()), t))
                    .children(calls.rows.iter().map(|row| call_row(row, t)))
                    .when(!calls.complete, |list| {
                        list.child(mono(
                            "More calls were made than the result lists.",
                            Type::MICRO,
                            t.dim,
                        ))
                    }),
            )
        })
        .when_some(said, |body, said| {
            let lines: Vec<&str> = said.output.lines().collect();
            let more = lines.len().saturating_sub(OUTPUT_LINES);
            body.when(!said.output.is_empty(), |body| {
                body.child(heading("Output", t)).child(
                    mono(
                        lines[..lines.len() - more].join("\n"),
                        Type::CAPTION,
                        t.muted,
                    )
                    .when(more > 0, |text| {
                        text.child(mono(
                            format!("… {more} more lines"),
                            Type::MICRO,
                            t.dim,
                        ))
                    }),
                )
            })
            .when_some(said.error.clone(), |body, error| {
                body.child(mono(error, Type::CAPTION, t.red))
            })
        })
        .when_some(writes, |body, writes| {
            let mut parts = Vec::new();
            if !writes.set.is_empty() {
                let keys: Vec<&str> =
                    writes.set.keys().map(String::as_str).collect();
                parts.push(format!("stored {}", keys.join(", ")));
            }
            if !writes.delete.is_empty() {
                parts.push(format!("deleted {}", writes.delete.join(", ")));
            }
            body.child(mono(parts.join(" · "), Type::MICRO, t.dim))
        })
}

/// One call: how it went, the tool, its arguments, its time and cost;
/// its error and plugins' verdicts under it.
fn call_row(row: &Row, t: &Theme) -> Div {
    let blocked = row.marks.iter().find_map(|mark| match &mark.mark {
        CardMark::Blocked { reason } => Some((mark.plugin.clone(), reason)),
        CardMark::Flagged => None,
    });
    let flagged = row
        .marks
        .iter()
        .find(|mark| mark.mark == CardMark::Flagged)
        .map(|mark| mark.plugin.clone());
    let status = match row.status {
        _ if blocked.is_some() => icon(Icon::Blocked, IconSize::SMALL, t.red),
        CallStatus::Running => icon(Icon::Spinner, IconSize::SMALL, t.accent),
        CallStatus::Ok => icon(Icon::Check, IconSize::SMALL, t.green),
        CallStatus::Error => icon(Icon::Blocked, IconSize::SMALL, t.red),
        CallStatus::Cancelled => icon(Icon::Close, IconSize::SMALL, t.dim),
    };
    let when = match (row.status, row.ms) {
        (CallStatus::Cancelled, _) => "cancelled".to_owned(),
        (_, Some(ms)) => format!("{ms} ms"),
        (_, None) => String::new(),
    };
    div()
        .flex()
        .flex_col()
        .py(sp(1.))
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(status)
                .child(
                    mono(row.name.clone(), Type::CAPTION, t.blue)
                        .flex_shrink_0(),
                )
                .child(
                    mono(row.args.clone(), Type::MICRO, t.dim)
                        .flex_1()
                        .min_w(px(0.))
                        .truncate(),
                )
                .children(row.cost.filter(|cost| *cost > 0.0).map(|cost| {
                    mono(fine_usd(cost), Type::MICRO, t.dim).flex_shrink_0()
                }))
                .child(mono(when, Type::MICRO, t.dim).flex_shrink_0()),
        )
        .when_some(row.error.clone(), |row, error| {
            row.child(mono(error, Type::MICRO, t.red).pl(sp(6.)))
        })
        .when_some(blocked, |row, (plugin, reason)| {
            row.child(
                mono(
                    format!("blocked by {plugin}: {reason}"),
                    Type::MICRO,
                    t.red,
                )
                .pl(sp(6.)),
            )
        })
        .when_some(flagged, |row, plugin| {
            row.child(
                mono(format!("flagged by {plugin}"), Type::MICRO, t.accent)
                    .pl(sp(6.)),
            )
        })
}

/// The store a run's scripts kept, key by key.
fn store_section(state: &State, t: &Theme) -> Div {
    let inference = state.inference_statuses();
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(heading(
            &format!("Codemode store · {}", state.store.len()),
            t,
        ))
        .child(if state.store.is_empty() {
            ui::text(
                "Its scripts deleted everything they stored.",
                Type::CAPTION,
                t.muted,
            )
        } else {
            ui::key_values(
                state.store.iter().map(|(key, value)| {
                    (
                        key.clone().into(),
                        mono(
                            preview(&value.to_string(), MAX_ARGS_CHARS),
                            Type::CAPTION,
                            t.text_soft,
                        ),
                    )
                }),
                t,
            )
        })
        .when(!inference.is_empty(), |section| {
            section.child(heading("Inference traces", t)).children(
                inference.into_iter().map(|(id, status)| {
                    mono(format!("{id} · {status}"), Type::CAPTION, t.text_soft)
                }),
            )
        })
}
