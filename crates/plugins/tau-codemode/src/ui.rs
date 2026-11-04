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
//!
//! Each is a view tree (ADR 0045): an output item, a trace, a module's
//! source and tests and a promotion's exact request are sections, open
//! or closed as the interface keeps them.

use std::{collections::BTreeMap, io::Write};

use gpui::{AnyElement, App};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ui_kit::{format::fine_usd, theme};
use tau_ui_plugin::{
    CallData,
    CardMark,
    Fold,
    Handle,
    Manifest,
    NestedMark,
    PluginStatus,
    RunCx,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRun, CardView},
    tree::{KeyValue, Kind, Node, Tone},
};

use crate::{
    CallStatus,
    PLUGIN,
    description::NAME,
    format::formatted,
    live::{InferUpdate, JevUpdate},
    modules,
    outline,
    promotion,
    result::{MAX_ARGS_CHARS, MAX_ERROR_CHARS, preview},
    store::{Record, Snapshot, Writes},
};

/// tau-codemode with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodemodeUi;

/// A user request to select an already persisted immutable version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Select {
        run: RunId,
        name: String,
        version: String,
    },
    Promote {
        run: RunId,
        request_id: String,
        decision: promotion::Decision,
    },
}

/// What the host half answers an [`Action`] it could not carry out.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionReply {
    pub error: String,
}

/// Validate against the run's persisted fork records, never the view state.
pub fn validate_selection(
    records: &[Value],
    name: &str,
    version: &str,
) -> Result<modules::Record, String> {
    for value in records {
        if value.get("kind").and_then(Value::as_str) == Some("module")
            && value.get("op").and_then(Value::as_str) == Some("define")
            && value.pointer("/definition/version").and_then(Value::as_str)
                == Some(version)
            && value.pointer("/definition/name").and_then(Value::as_str)
                == Some(name)
        {
            let record: Record = serde_json::from_value(value.clone())
                .map_err(|_| {
                    "The saved module definition is corrupt".to_owned()
                })?;
            if let Record::Module(modules::Record::Define { definition }) =
                record
            {
                definition.verify().map_err(|error| {
                    format!("The saved module definition is corrupt: {error}")
                })?;
            }
        }
    }
    let library = modules::fold(records);
    match library.versions().get(version) {
        Some(definition) if definition.name() == name => {
            Ok(modules::Record::Select {
                name: name.to_owned(),
                version: version.to_owned(),
            })
        }
        Some(_) => Err("That version belongs to another module".into()),
        None => Err("That module version is missing from this run".into()),
    }
}

/// The inspector's button sends only run, name, and exact version.
pub fn select_version(
    run: &RunId,
    name: &str,
    version: &str,
    handle: &Handle,
    cx: &mut App,
) {
    handle.act(
        Action::Select {
            run: run.clone(),
            name: name.to_owned(),
            version: version.to_owned(),
        },
        cx,
    );
}

/// The button sends only the persisted request identity and decision.
pub fn decide_promotion(
    run: &RunId,
    request_id: &str,
    decision: promotion::Decision,
    handle: &Handle,
    cx: &mut App,
) {
    handle.act(
        Action::Promote {
            run: run.clone(),
            request_id: request_id.to_owned(),
            decision,
        },
        cx,
    );
}

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
    /// Full immutable repository source snapshots, keyed by owning run ID.
    pub repository_pins: BTreeMap<String, modules::RepositoryPin>,
    /// Durable promotion requests and terminal decisions, suitable for phones.
    pub promotions: Vec<promotion::Record>,
    /// Serialized size of the folded promotion records.
    pub promotion_bytes: usize,
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
            Record::RepositoryPin(pin) => {
                if pin.verify().is_ok() {
                    self.repository_pins.insert(pin.owner.clone(), pin);
                }
            }
            Record::Promotion(record) => {
                if self.promotion_bytes == 0 && !self.promotions.is_empty() {
                    self.promotion_bytes = self
                        .promotions
                        .iter()
                        .map(|item| {
                            serde_json::to_vec(item)
                                .map_or(0, |bytes| bytes.len())
                        })
                        .sum();
                }
                if self.promotions.len() < promotion::MAX_REQUESTS * 2 {
                    let size = serde_json::to_vec(&record)
                        .map_or(usize::MAX, |bytes| bytes.len());
                    if self.promotion_bytes.saturating_add(size)
                        <= promotion::MAX_FOLDED_BYTES
                    {
                        self.promotion_bytes += size;
                        self.promotions.push(record);
                    }
                }
            }
            Record::Inference(record) => self.inference.push(record),
        }
    }
}

impl State {
    /// A start stays incomplete until a matching durable finish arrives.
    pub fn inference_statuses(&self) -> BTreeMap<String, &'static str> {
        inference_traces(self)
            .into_iter()
            .map(|(id, trace)| (id, trace.status()))
            .collect()
    }
}

/// The durable records belonging to one private inference trace.
#[derive(Default)]
pub struct TraceView<'a> {
    pub started: Option<&'a crate::inference_trace::Record>,
    pub reservations: Vec<u32>,
    pub finished: Option<&'a crate::inference_trace::Record>,
}

impl TraceView<'_> {
    pub fn status(&self) -> &'static str {
        match (self.started, self.finished) {
            (Some(_), Some(crate::inference_trace::Record::Finished { complete: true, error: None, attempts, .. }))
                if attempts.iter().map(|attempt| attempt.number).eq(self.reservations.iter().copied())
                    && attempts.iter().all(|attempt| attempt.outcome == crate::inference_trace::AttemptOutcome::Finished
                        && matches!(attempt.usage_provenance, crate::inference_trace::UsageProvenance::SdkReported | crate::inference_trace::UsageProvenance::SdkZeroOrDefault)) => "finished",
            (Some(_), Some(crate::inference_trace::Record::Finished { complete: true, error: Some(_), attempts, .. }))
                if attempts.iter().map(|attempt| attempt.number).eq(self.reservations.iter().copied())
                    && attempts.iter().all(|attempt| attempt.outcome == crate::inference_trace::AttemptOutcome::Finished
                        && matches!(attempt.usage_provenance, crate::inference_trace::UsageProvenance::SdkReported | crate::inference_trace::UsageProvenance::SdkZeroOrDefault)) => "failed",
            _ => "interrupted / incomplete",
        }
    }
}

/// Group the already folded records; a missing phase remains visible.
pub fn inference_traces(state: &State) -> BTreeMap<String, TraceView<'_>> {
    let mut traces = BTreeMap::<String, TraceView<'_>>::new();
    for record in &state.inference {
        let trace = traces.entry(record.trace_id().to_owned()).or_default();
        match record {
            crate::inference_trace::Record::Started { .. } => {
                trace.started = Some(record)
            }
            crate::inference_trace::Record::Attempt { number, .. } => {
                trace.reservations.push(*number)
            }
            crate::inference_trace::Record::Finished { .. } => {
                trace.finished = Some(record)
            }
        }
    }
    traces
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
    /// Reported US dollars for Jev or all admitted infer attempts.
    pub cost: Option<f64>,
    pub usage_uncertain: bool,
    /// Latest nested infer progress, while its call is still running.
    pub progress: Option<String>,
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
                usage_uncertain: false,
                progress: (status == CallStatus::Running)
                    .then(|| call.partial.clone())
                    .flatten(),
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
    // ToolEnd events and Codemode updates use independent channels. The
    // infer update is the authoritative full row, whichever arrived first.
    for update in data.updates.iter().filter_map(InferUpdate::from_details) {
        let Some(mut latest) = row_of(&update.row) else {
            continue;
        };
        if latest.status == CallStatus::Running {
            latest.ms = None;
        }
        if let Some((_, row)) =
            keyed.iter_mut().find(|(_, row)| row.id == latest.id)
        {
            latest.progress = row.progress.take();
            if latest.status != CallStatus::Running {
                latest.progress = None;
            }
            *row = latest;
        }
    }
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
        usage_uncertain: row
            .get("usage_uncertain")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        progress: None,
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

/// Why a finished script failed, from its result's text: the
/// `Script error:` item's message. A call that failed before the script
/// ran (a bad options line) has only its message.
pub fn failure(text: &str, failed: bool) -> Option<String> {
    if !failed {
        return None;
    }
    let Some((_, rest)) = text.split_once("\nOutput:\n") else {
        return Some(text.trim().to_owned());
    };
    let (_, error) = rest.rsplit_once("Script error:\n")?;
    // The list of calls made before it is the card's rows.
    let head = error.split("\n\n").next().unwrap_or(error);
    Some(head.trim().to_owned())
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

/// Reported Jev and inference cost, from finished details' `usage`.
pub fn cost(details: &Value) -> f64 {
    details["usage"]["cost"]["total"].as_f64().unwrap_or(0.0)
}

/// The header's line: `3 calls · 1.2 s · $0.0004`. While the script
/// runs (no details yet), the calls so far and their reported cost.
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
    if calls.rows.iter().any(|row| row.usage_uncertain) {
        parts.push("reported partial · final usage unknown".to_owned());
    }
    parts.join(" · ")
}

impl UiPlugin for CodemodeUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        PLUGIN
    }

    fn reply(&self, reply: Value, handle: &Handle, cx: &mut App) {
        if let Ok(reply) = serde_json::from_value::<ActionReply>(reply) {
            let title = if reply.error.starts_with("Promotion:") {
                "Module promotion failed"
            } else {
                "Module selection failed"
            };
            handle.alert(title, reply.error, cx);
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::CARD, card)
            .contribute(points::INSPECTOR, inspector)
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
                    tone: theme::Tone::Quiet,
                })
            })
    }
}

/// Every button's action: carry out the [`Action`] its arguments hold.
const ACT: &str = "act";

/// A button that asks the host half for `action`.
fn button(label: &str, action: &Action) -> Node {
    Node::Button {
        label: label.into(),
        action: ACT.into(),
        args: serde_json::to_value(action).unwrap_or(Value::Null),
        kind: Kind::Secondary,
        icon: None,
    }
}

/// What a tree's buttons ask: the action in their arguments.
fn on_act(handle: Handle) -> impl Fn(&str, &Value, &mut App) + 'static {
    move |action, args, cx| {
        if action != ACT {
            return;
        }
        if let Ok(action) = serde_json::from_value::<Action>(args.clone()) {
            handle.act(action, cx);
        }
    }
}

fn icon(name: &str, tone: Tone) -> Node {
    Node::Icon {
        name: name.into(),
        tone,
    }
}

fn mono(text: impl Into<String>) -> Node {
    Node::Mono { text: text.into() }
}

fn heading(text: impl Into<String>) -> Node {
    Node::Heading { text: text.into() }
}

fn code(lang: &str, text: impl AsRef<str>) -> Node {
    Node::Code {
        lang: Some(lang.into()),
        text: text.as_ref().to_owned(),
        first_line: None,
    }
}

/// A line marked by an icon in its tone: an error, a verdict.
fn marked(name: &str, tone: Tone, text: impl Into<String>) -> Node {
    Node::row([icon(name, tone), mono(text)])
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
    let data = &at.data;
    let calls = calls(&at.call_id, data);
    let details = data.result.as_ref().and_then(|r| r.details.as_ref());
    let failed = data
        .result
        .as_ref()
        .and_then(|result| failure(&result.text, result.error));
    let code = script(data);
    let ignore = |_: &str, _: &Value, _: &mut App| {};
    let head = view.tree(
        &format!("codemode-card-head-{}", at.call_id),
        &mono(first_line(code)),
        ignore,
    );
    let body = view.tree(
        &format!("codemode-card-body-{}", at.call_id),
        &body(&at.call_id, code, &calls, failed.clone(), details),
        ignore,
    );
    Some(CardView {
        head: Some(head),
        label: Some(label(&calls, details)),
        failed: failed
            .as_ref()
            .map(|error| error.lines().next().unwrap_or_default().to_owned()),
        edge: failed.as_ref().map(|_| theme::Tone::Danger),
        shape: None,
        body: Some(body),
        // Open while it runs, so its calls show as they come; closed once
        // it ended, down to its header.
        folds: data.result.is_some(),
        inset: false,
    })
}

/// The card's body: the script, its calls, its output (each item a
/// section that opens), why it failed and what it stored.
pub fn body(
    call_id: &str,
    code: &str,
    calls: &Calls,
    failed: Option<String>,
    details: Option<&Value>,
) -> Node {
    let mut body = vec![self::code("luau", formatted(code))];
    if !calls.rows.is_empty() {
        body.push(heading(format!("Calls · {}", calls.rows.len())));
        body.push(Node::Table {
            head: Vec::new(),
            rows: calls.rows.iter().map(call_row).collect(),
            widths: vec![0.4, 2., 6., 1.4, 1.2],
        });
        if !calls.complete {
            body.push(Node::text(
                "More calls were made than the result lists.",
            ));
        }
    }
    let items = details.map(outline::items).unwrap_or_default();
    if !items.is_empty() {
        body.push(heading("Output"));
        body.push(outline::outline(call_id, &items));
    }
    if let Some(error) = failed {
        body.push(marked("blocked", Tone::Bad, error));
    }
    let writes = details
        .and_then(|details| details.get("store"))
        .and_then(|store| serde_json::from_value::<Writes>(store.clone()).ok())
        .filter(|writes| !writes.is_empty());
    if let Some(writes) = writes {
        let mut parts = Vec::new();
        if !writes.set.is_empty() {
            let keys: Vec<&str> =
                writes.set.keys().map(String::as_str).collect();
            parts.push(format!("stored {}", keys.join(", ")));
        }
        if !writes.delete.is_empty() {
            parts.push(format!("deleted {}", writes.delete.join(", ")));
        }
        body.push(Node::text(parts.join(" · ")));
    }
    Node::stack(body)
}

/// One call's row: how it went, the tool, its arguments with its error
/// and plugins' verdicts under them, its cost and its time.
fn call_row(row: &Row) -> Vec<Node> {
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
        _ if blocked.is_some() => icon("blocked", Tone::Bad),
        CallStatus::Running => icon("spinner", Tone::Info),
        CallStatus::Ok => icon("check", Tone::Good),
        CallStatus::Error => icon("blocked", Tone::Bad),
        CallStatus::Cancelled => icon("close", Tone::Neutral),
    };
    let when = match (row.status, row.ms) {
        (CallStatus::Cancelled, _) => "cancelled".to_owned(),
        (_, Some(ms)) => format!("{ms} ms"),
        (_, None) => String::new(),
    };
    let cost = row
        .cost
        .filter(|cost| *cost > 0.0)
        .map(|cost| {
            format!(
                "{}{}",
                fine_usd(cost),
                if row.usage_uncertain {
                    " reported partial"
                } else {
                    ""
                }
            )
        })
        .unwrap_or_default();
    let mut what = vec![mono(row.args.clone())];
    if let Some(error) = &row.error {
        what.push(marked("blocked", Tone::Bad, error.clone()));
    }
    if let Some(progress) = &row.progress {
        what.push(Node::text(progress.clone()));
    }
    if let Some((plugin, reason)) = blocked {
        what.push(marked(
            "blocked",
            Tone::Bad,
            format!("blocked by {plugin}: {reason}"),
        ));
    }
    if let Some(plugin) = flagged {
        what.push(marked(
            "warning",
            Tone::Warn,
            format!("flagged by {plugin}"),
        ));
    }
    vec![
        status,
        mono(row.name.clone()),
        Node::stack(what),
        Node::text(cost),
        Node::text(when),
    ]
}

/// The inspector: the store a run's scripts kept, its repository's
/// modules, its own, and its promotion requests.
pub fn inspector(
    _: &AtRun,
    view: &mut ViewCx<'_, CodemodeUi>,
) -> Option<AnyElement> {
    let state = view.state?;
    let run = view.run.map(|run| run.id.clone());
    let tree = inspector_tree(state, run.as_ref());
    let handle = view.handle.clone();
    let id = run.as_ref().map_or(String::new(), ToString::to_string);
    Some(view.tree(&format!("codemode-inspector-{id}"), &tree, on_act(handle)))
}

/// The inspector's tree for `state`, in `run` when there is one.
pub fn inspector_tree(state: &State, run: Option<&RunId>) -> Node {
    let mut sections = vec![store_section(state, run)];
    if let Some((run, pin)) = run
        .and_then(|run| Some((run, state.repository_pins.get(run.0.as_ref())?)))
    {
        sections.push(repository_section(pin, run));
    }
    sections.push(modules_section(&state.modules, run));
    if let Some(run) = run {
        sections.push(promotions_section(&state.promotions, run));
    }
    Node::stack(sections)
}

/// The store a run's scripts kept, key by key, and its scripts' private
/// inference traces, each a section that opens to what it holds.
fn store_section(state: &State, run: Option<&RunId>) -> Node {
    let run = run.map_or(String::new(), ToString::to_string);
    let mut section =
        vec![heading(format!("Codemode store · {}", state.store.len()))];
    section.push(if state.store.is_empty() {
        Node::text("Its scripts deleted everything they stored.")
    } else {
        Node::Rows {
            rows: state
                .store
                .iter()
                .map(|(key, value)| KeyValue {
                    key: key.clone(),
                    value: preview(&value.to_string(), MAX_ARGS_CHARS),
                })
                .collect(),
        }
    });
    let inference = inference_traces(state);
    if !inference.is_empty() {
        section.push(heading("Inference traces"));
        section.extend(inference.into_iter().map(|(id, trace)| {
            Node::Section {
                id: format!("codemode-trace-{run}-{id}"),
                title: format!("{id} · {}", trace.status()),
                open: false,
                children: trace_sections(&trace, true)
                    .into_iter()
                    .flat_map(|(name, value)| {
                        [
                            heading(name),
                            Node::Code {
                                lang: None,
                                text: value,
                                first_line: None,
                            },
                        ]
                    })
                    .collect(),
            }
        }));
    }
    Node::stack(section)
}

/// The selected version first, followed by each previous immutable version.
fn modules_section(library: &modules::Library, run: Option<&RunId>) -> Node {
    let mut section = vec![heading(format!(
        "Codemode modules · {}",
        library.selected().len()
    ))];
    if library.selected().is_empty() {
        section.push(Node::text("No modules defined in this run."));
        return Node::stack(section);
    }
    let Some(run) = run else {
        return Node::stack(section);
    };
    for (name, selected) in library.selected() {
        section.push(heading(name.clone()));
        if let Some(definition) = library.versions().get(selected) {
            section.push(version_row(
                definition,
                true,
                library.tests(selected),
                Scope::Run(run),
            ));
        }
        let previous: Vec<_> = library
            .versions()
            .values()
            .filter(|definition| {
                definition.name() == name && definition.version() != selected
            })
            .collect();
        if !previous.is_empty() {
            section.push(Node::text("Previous immutable versions"));
            for definition in previous {
                section.push(version_row(
                    definition,
                    false,
                    library.tests(definition.version()),
                    Scope::Run(run),
                ));
            }
        }
    }
    Node::stack(section)
}

/// Draw only definitions captured in this run's persisted repository pin.
fn repository_section(pin: &modules::RepositoryPin, run: &RunId) -> Node {
    let mut section = vec![heading(format!(
        "Repository modules · {} selected",
        pin.selected.len()
    ))];
    for (name, version) in &pin.selected {
        if let Some(definition) = pin.resolve(name, Some(version)) {
            section.push(heading(name.clone()));
            section.push(version_row(definition, true, &[], Scope::Pin(run)));
        }
    }
    for definition in pin.versions.values() {
        if pin
            .selected
            .get(definition.name())
            .is_some_and(|version| version == definition.version())
        {
            continue;
        }
        section.push(heading(definition.name().to_owned()));
        section.push(version_row(definition, false, &[], Scope::Pin(run)));
    }
    Node::stack(section)
}

fn promotions_section(records: &[promotion::Record], run: &RunId) -> Node {
    let requests: Vec<_> = records
        .iter()
        .filter_map(|record| match record {
            promotion::Record::Requested(request)
                if request.owner == run.0.as_ref() =>
            {
                Some(request.as_ref())
            }
            _ => None,
        })
        .collect();
    let mut section = vec![heading(format!(
        "Repository promotion requests · {}",
        requests.len()
    ))];
    for request in requests {
        let decisions: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                promotion::Record::Decided(terminal)
                    if terminal.request_id == request.id
                        && terminal.owner == request.owner
                        && terminal.digest == request.digest =>
                {
                    Some(terminal.decision)
                }
                _ => None,
            })
            .collect();
        let status = match decisions.as_slice() {
            [] => "Pending",
            [promotion::Decision::Approved] => "Approved",
            [promotion::Decision::Declined] => "Declined",
            _ => "Conflicting",
        };
        let mut exact = vec![
            Node::text(format!(
                "Repository: {} · key {}",
                request.repository_scope, request.repository_key
            )),
            Node::text(format!("Content digest: {}", request.digest)),
            Node::text(format!(
                "Dependencies: {}",
                compact_json_preview(request.root.dependencies(), 2048)
            )),
            Node::text(format!(
                "Closure: {}",
                compact_json_preview(&request.versions, 4096)
            )),
            Node::text(format!("Test evidence: {}", request.tests.len())),
            code("luau", formatted(request.root.source())),
            code(
                "json",
                compact_json_preview(request.root.signatures(), 16 * 1024),
            ),
        ];
        exact.extend(
            request
                .tests
                .iter()
                .map(|test| code("json", compact_json_preview(test, 4096))),
        );
        let mut card = vec![
            mono(format!(
                "{status} · {}@{} · {}",
                request.root.name(),
                request.root.version(),
                request.id
            )),
            Node::Section {
                id: format!("codemode-promotion-{run}-{}", request.id),
                title: "Show exact request".into(),
                open: false,
                children: exact,
            },
        ];
        if status == "Pending" {
            card.push(Node::row(
                [
                    ("Approve", promotion::Decision::Approved),
                    ("Decline", promotion::Decision::Declined),
                ]
                .map(|(label, decision)| {
                    button(
                        label,
                        &Action::Promote {
                            run: run.clone(),
                            request_id: request.id.clone(),
                            decision,
                        },
                    )
                }),
            ));
        }
        section.push(Node::Card {
            children: vec![Node::stack(card)],
        });
    }
    Node::stack(section)
}

/// Where a module version is listed: among the run's own, which it can
/// select and which have tests, or in its repository's pin.
#[derive(Clone, Copy)]
enum Scope<'a> {
    Run(&'a RunId),
    Pin(&'a RunId),
}

/// One module version: what it is, its source, signatures and tests,
/// each a section that opens; a previous one of the run's own can be
/// selected.
fn version_row(
    definition: &modules::Definition,
    selected: bool,
    tests: &[modules::ModuleTest],
    scope: Scope<'_>,
) -> Node {
    let name = definition.name();
    let version = definition.version();
    let (run, own) = match scope {
        Scope::Run(run) => (run, true),
        Scope::Pin(run) => (run, false),
    };
    let id = |what: &str| {
        format!(
            "codemode-module-{}-{run}-{what}-{version}",
            if own { "run" } else { "pin" }
        )
    };
    let mut header = vec![mono(format!(
        "{} · {}",
        if selected { "Selected" } else { "Version" },
        version
    ))];
    if !selected && own {
        header.push(Node::Spacer);
        header.push(button(
            "Select version",
            &Action::Select {
                run: run.clone(),
                name: name.to_owned(),
                version: version.to_owned(),
            },
        ));
    }
    // Compact definitions are bounded at 16 KiB; no pretty-print
    // amplification.
    let signatures = serde_json::to_string(definition.signatures())
        .expect("signatures are JSON");
    let mut row = vec![
        Node::row(header),
        Node::text(format!(
            "Signatures: {}",
            compact_json_preview(definition.signatures(), 1024)
        )),
        Node::text(format!(
            "Dependencies: {}",
            if definition.dependencies().is_empty() {
                "none".to_owned()
            } else {
                definition
                    .dependencies()
                    .iter()
                    .map(|(name, version)| format!("{name}@{version}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        )),
        Node::Section {
            id: id("source"),
            title: "Show source".into(),
            open: false,
            children: vec![code("luau", formatted(definition.source()))],
        },
        Node::Section {
            id: id("signatures"),
            title: "Show signatures".into(),
            open: false,
            children: vec![code("json", signatures)],
        },
    ];
    if own {
        row.push(mono(format!("Controlled tests · {}", tests.len())));
        row.push(Node::text(
            "Tests use supplied fake calls and exact module versions; \
             passing is evidence for those inputs, not proof of live \
             behavior.",
        ));
        for (index, test) in tests.iter().enumerate() {
            row.push(test_section(
                test,
                index,
                version,
                id(&format!("test-{index}")),
            ));
        }
    }
    Node::Card {
        children: vec![Node::stack(row)],
    }
}

/// A module's test: whether it passed, and, open, its script, the fake
/// calls it was given, what it returned and called, and its error.
fn test_section(
    test: &modules::ModuleTest,
    index: usize,
    version: &str,
    id: String,
) -> Node {
    let report = test.result();
    let mut children = vec![
        code("luau", formatted(test.code())),
        Node::text(format!(
            "Fake calls: {}",
            compact_json_preview(&test.tools(), 2048)
        )),
        Node::text(format!(
            "Output{}: {}",
            if report.output_truncated {
                " (truncated)"
            } else {
                ""
            },
            preview(&report.output, 2048)
        )),
        Node::text(format!(
            "Calls: {}",
            compact_json_preview(&report.calls, 2048)
        )),
    ];
    if let Some(error) = &report.error {
        children.push(marked(
            "blocked",
            Tone::Bad,
            format!(
                "Error{}: {}",
                if report.error_truncated {
                    " (truncated)"
                } else {
                    ""
                },
                preview(error, 2048)
            ),
        ));
    }
    Node::Section {
        id,
        title: format!(
            "{} · test {} · {}",
            if report.passed { "Passed" } else { "Failed" },
            index + 1,
            version
        ),
        open: false,
        children,
    }
}

/// Compact JSON written into a byte cap; a large fake fixture never gets
/// expanded into pretty JSON just to draw the inspector.
pub fn compact_json_preview(value: &impl Serialize, limit: usize) -> String {
    struct Limited(Vec<u8>, usize, bool);
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let remaining = self.1.saturating_sub(self.0.len());
            self.0
                .extend_from_slice(&bytes[..bytes.len().min(remaining)]);
            if bytes.len() > remaining {
                self.2 = true;
                Err(std::io::Error::other("preview limit"))
            } else {
                Ok(bytes.len())
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Limited(Vec::new(), limit, false);
    let _ = serde_json::to_writer(&mut out, value);
    let mut text = String::from_utf8_lossy(&out.0).into_owned();
    if out.2 {
        text.push('…');
    }
    text
}

/// Text rendered by the inspector's trace fold. Closed folds expose none
/// of the private fields. Compact JSON avoids pretty-print amplification.
pub fn trace_sections(
    trace: &TraceView<'_>,
    open: bool,
) -> Vec<(&'static str, String)> {
    if !open {
        return Vec::new();
    }
    let mut sections = Vec::new();
    if let Some(crate::inference_trace::Record::Started {
        task,
        context,
        schema,
        model,
        effort,
        ..
    }) = trace.started
    {
        sections.push((
            "Model",
            format!(
                "{model} · reasoning: {}",
                effort.as_deref().unwrap_or("default")
            ),
        ));
        sections.push(("Task", task.clone()));
        sections.push(("Context", context.to_string()));
        if let Some(schema) = schema {
            sections.push(("Schema", schema.to_string()));
        }
    } else {
        sections.push((
            "Trace",
            "Missing start record; task and context unavailable.".into(),
        ));
    }
    if let Some(crate::inference_trace::Record::Finished {
        attempts,
        total_usage,
        selected,
        raw_output,
        raw_output_truncated,
        error,
        complete,
        ..
    }) = trace.finished
    {
        sections.push((
            "Attempts",
            serde_json::to_string(attempts).unwrap_or_default(),
        ));
        sections.push((
            "Reported usage",
            format!(
                "{} tokens · {}{}",
                total_usage.total_tokens,
                fine_usd(total_usage.cost.total),
                if *complete {
                    ""
                } else {
                    " · final usage unknown"
                }
            ),
        ));
        if attempts.iter().any(|attempt| {
            attempt.usage_provenance
                == crate::inference_trace::UsageProvenance::SdkZeroOrDefault
        }) {
            sections.push(("Usage provenance", "SDK zero or default usage does not establish zero provider usage.".into()));
        }
        sections.push((
            "Raw answer",
            raw_output
                .clone()
                .unwrap_or_else(|| "Raw answer unavailable.".into()),
        ));
        if *raw_output_truncated {
            sections.push((
                "Raw answer limit",
                "Raw answer was truncated in the trace.".into(),
            ));
        }
        if let Some(selected) = selected {
            sections.push(("Selected value", selected.to_string()));
        }
        if let Some(error) = error {
            sections.push(("Error", error.clone()));
        }
    } else {
        sections.push(("Trace", format!("{} attempt reservations · no terminal trace; final usage unknown.", trace.reservations.len())));
    }
    sections
}
