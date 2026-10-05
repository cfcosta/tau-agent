//! fast-compaction's UI (ADR 0017): what its passes left of each tool
//! call, on the call's card and on its ledger page; each pass where it
//! rewrote the transcript; where it steps in on the context meter; and
//! its line in the run's plugin list.

use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt as _, sync::Arc};

use gpui::{AnyElement, Div, SharedString, div, prelude::*, relative, rems};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, bar, heading, icon, key_values, link, mono},
    format::{fine_usd, tokens},
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
};
use tau_ui_plugin::{
    Dropped,
    Fold,
    HostCx,
    Link,
    Manifest,
    NO_KEY,
    OutputCut,
    Page,
    PluginInfo,
    RunCtx,
    RunCx,
    RunInfo,
    Seam,
    UiPlugin,
    ViewCx,
    needs_jev,
    points::{self, AtCard, AtRewrite, AtRun},
};

use crate::{
    Action,
    Details,
    FastCompaction,
    NAME,
    OutputStats,
    Record,
    Settings,
};

/// fast-compaction with its UI: what tau adds to an agent.
#[derive(Debug, Clone, Copy, Default)]
pub struct FastCompactionUi;

/// What fast-compaction did in one run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// Whether it runs with the run: there was a TypeSafe key when it
    /// started or went on.
    pub on: Option<bool>,
    /// Each pass that rewrote the transcript, by the key it named it.
    pub passes: BTreeMap<String, Pass>,
    /// The key of the last pass.
    pub last: Option<String>,
    /// What the last pass decided for each tool call, in order.
    pub ledger: Vec<Entry>,
    /// Outputs cut as they arrived, and the tokens that saved.
    pub outputs: usize,
    pub outputs_saved: u64,
}

/// One pass that rewrote the transcript.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pass {
    /// The transcript's estimated tokens before and after.
    pub before: u64,
    pub after: u64,
    pub results_dropped: usize,
    pub calls_dropped: usize,
    pub reduction_ratio: f64,
    /// What it judged and what Jev cost: `12 calls judged in 1 Jev
    /// request · …`.
    pub detail: String,
}

/// One tool call as the last pass judged it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub call_id: String,
    pub turn: u32,
    pub tool: String,
    pub input: String,
    /// Tokens the call and its result take.
    pub tokens: u64,
    /// The probability that knowing the call still matters.
    pub matters: Option<f32>,
    /// The probability that its full output must stay verbatim.
    pub verbatim: Option<f32>,
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Decision {
    /// Too recent to judge.
    Pinned,
    Keep,
    DropResult,
    DropCall,
}

impl Decision {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Keep => "keep",
            Self::DropResult => "drop result",
            Self::DropCall => "drop call",
        }
    }

    /// What the call's card says of it.
    pub fn card_label(self) -> &'static str {
        match self {
            Self::Pinned | Self::Keep => "kept",
            Self::DropResult => "result dropped",
            Self::DropCall => "call dropped",
        }
    }
}

impl From<Action> for Decision {
    fn from(action: Action) -> Self {
        match action {
            Action::Keep => Self::Keep,
            Action::DropResult => Self::DropResult,
            Action::DropCall => Self::DropCall,
        }
    }
}

impl Fold for State {
    type Record = Record;

    /// Folds one of the plugin's reports, or what it says as a run
    /// starts.
    fn apply(&mut self, record: Record, run: &mut dyn RunCx) {
        match record {
            Record::Starting { on } => self.on = Some(on),
            Record::Output(stats) => self.output(stats, run),
            Record::Ledger(details) => self.pass(&details, run),
        }
    }

    /// A stored rewrite's details: the pass that made it.
    fn rewritten(&mut self, details: Value, run: &mut dyn RunCx) {
        if let Ok(details) = serde_json::from_value::<Details>(details) {
            self.pass(&details, run);
        }
    }
}

impl State {
    /// An output cut as it arrived: the call's card says what the model
    /// saw, and where the whole output is.
    fn output(&mut self, stats: OutputStats, run: &mut dyn RunCx) {
        let Some(archive) = stats.archive.filter(|_| stats.pruned) else {
            return;
        };
        let cut = OutputCut {
            kept: stats.lines.saturating_sub(stats.dropped_lines),
            lines: stats.lines,
            archive,
            tokens_before: stats.tokens_before as u64,
            tokens_after: stats.tokens_after as u64,
            cost: stats.cost,
        };
        self.outputs += 1;
        self.outputs_saved +=
            cut.tokens_before.saturating_sub(cut.tokens_after);
        run.cut(&stats.call_id, cut);
    }

    /// A pass: its ledger, each call it dropped marked on its card, and
    /// the rewrite it makes, named.
    fn pass(&mut self, details: &Details, run: &mut dyn RunCx) {
        let decision = |call_id: &str| {
            details
                .decisions
                .iter()
                .find(|decision| decision.call_id == call_id)
        };
        let cards = run.cards();
        // Calls the pass dropped from a stored transcript have no card
        // left; they still show, first, as they came first.
        let mut ledger: Vec<Entry> = details
            .decisions
            .iter()
            .filter(|decision| {
                !cards.iter().any(|card| card.call_id == decision.call_id)
            })
            .map(|decision| Entry {
                call_id: decision.call_id.clone(),
                turn: 0,
                tool: decision.tool.clone(),
                input: String::new(),
                tokens: 0,
                matters: Some(decision.keep_call as f32),
                verbatim: Some(decision.keep_result as f32),
                decision: decision.action.into(),
            })
            .collect();
        for card in &cards {
            let decided = decision(&card.call_id);
            ledger.push(Entry {
                call_id: card.call_id.clone(),
                turn: card.turn,
                tool: card.tool.clone(),
                input: card.summary.clone(),
                tokens: (card.size / 4) as u64,
                matters: decided.map(|d| d.keep_call as f32),
                verbatim: decided.map(|d| d.keep_result as f32),
                decision: decided.map_or(Decision::Pinned, |d| d.action.into()),
            });
        }
        for entry in &ledger {
            match entry.decision {
                Decision::DropResult => {
                    run.dropped(&entry.call_id, Dropped::Result);
                }
                Decision::DropCall => {
                    run.dropped(&entry.call_id, Dropped::Call);
                }
                _ => {}
            }
            run.attach(&entry.call_id, &entry.call_id);
        }
        let stats = &details.stats;
        let count = |n: usize, one: &str, many: &str| {
            format!("{n} {}", if n == 1 { one } else { many })
        };
        let mut detail = format!(
            "{} judged in {} · {} cut, {} dropped · −{:.0}% · Jev read {} \
             tokens of history, {}",
            count(stats.calls - stats.pinned, "call", "calls"),
            count(stats.requests, "Jev request", "Jev requests"),
            count(stats.results_dropped, "result", "results"),
            count(stats.calls_dropped, "call", "calls"),
            stats.reduction_ratio * 100.0,
            tokens(stats.state_tokens as u64),
            stats.state_stage,
        );
        if stats.cost > 0.0 {
            detail.push_str(&format!(" · {}", fine_usd(stats.cost)));
        }
        let key = format!("p{}", self.passes.len());
        self.passes.insert(
            key.clone(),
            Pass {
                before: stats.chars_before.div_ceil(4) as u64,
                after: stats.chars_after.div_ceil(4) as u64,
                results_dropped: stats.results_dropped,
                calls_dropped: stats.calls_dropped,
                reduction_ratio: stats.reduction_ratio,
                detail,
            },
        );
        run.rewrite(&key);
        self.last = Some(key);
        self.ledger = ledger;
    }

    /// The last pass, if any.
    pub fn last_pass(&self) -> Option<&Pass> {
        self.passes.get(self.last.as_ref()?)
    }

    /// The plugin's line in the run's plugin list.
    pub fn status(&self) -> Option<String> {
        if let Some(pass) = self.last_pass() {
            return Some(format!(
                "{} pruned · −{:.0}%",
                pass.results_dropped + pass.calls_dropped,
                pass.reduction_ratio * 100.0
            ));
        }
        Some(match self.on? {
            true => "pruning large outputs · watching the window".to_owned(),
            false => NO_KEY.to_owned(),
        })
    }

    /// The last pass's decision on the call `call_id`.
    pub fn entry(&self, call_id: &str) -> Option<&Entry> {
        self.ledger.iter().find(|entry| entry.call_id == call_id)
    }

    /// How many calls the last pass made each decision for.
    pub fn count(&self, decision: Decision) -> usize {
        self.ledger
            .iter()
            .filter(|entry| entry.decision == decision)
            .count()
    }
}

/// Where a pass runs between turns, as a share of the window.
fn trigger() -> f32 {
    (Settings::default().compact_at_percent / 100.0) as f32
}

/// The ledger page of the run at hand.
fn ledger_link() -> Link {
    Link::page("ledger").param("run", "")
}

impl UiPlugin for FastCompactionUi {
    type State = State;
    type Data = ();
    type RepoData = ();
    type Settings = ();
    type Host = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    /// Pruning with Jev, when there is a key, on the run's model's
    /// window, archiving to the repository's directory in tau's.
    async fn agent_plugins(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let Some(jev) = run.services.get::<Arc<dyn Jev>>() else {
            return Ok(Vec::new());
        };
        let archive_dir = run.repo.dir.join("archive");
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&archive_dir);
        let settings = Settings {
            context_window: tau_ai::model::find(&run.model)
                .map(|model| model.context_window),
            archive_dir,
            ..Settings::default()
        };
        Ok(vec![Box::new(
            FastCompaction::shared(jev.clone()).settings(settings),
        )])
    }

    async fn starting(
        &self,
        _host: &(),
        run: &RunCtx,
        _settings: &(),
    ) -> Vec<Record> {
        let on = run.services.get::<Arc<dyn Jev>>().is_some();
        vec![Record::Starting { on }]
    }

    async fn catalog(
        &self,
        _host: &(),
        cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        let jev = cx.services.get::<Arc<dyn Jev>>().is_some();
        PluginInfo {
            description: needs_jev(
                jev,
                "Prunes large bash outputs as they arrive, and stale tool \
                 history",
            ),
            seams: vec![Seam::Start, Seam::Rewrite],
            page: Some(ledger_link()),
            ..Default::default()
        }
    }

    fn rewrites_keep_transcript(&self) -> bool {
        true
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new("ledger", ledger_page)
                    .title(|_| "Context ledger".to_owned()),
            )
            .status(State::status)
            .contribute(points::CONTEXT_TRIGGER, |_: &AtRun, view| {
                view.state?.on?.then(trigger)
            })
            .contribute(points::CONTEXT, context)
            .contribute(points::CARD_BADGE, badge)
            .contribute(points::REWRITE, rewrite)
    }
}

/// What the last pass made of a call, on its card: its size as the
/// pass weighed it, and what it kept.
fn badge(
    at: &AtCard,
    view: &mut ViewCx<'_, FastCompactionUi>,
) -> Option<AnyElement> {
    let entry = view.state?.entry(at.keys.first()?)?;
    // A result kept as it was needs no word: only what the pass changed
    // shows on the card.
    if matches!(entry.decision, Decision::Pinned | Decision::Keep) {
        return None;
    }
    let t = view.theme().clone();
    Some(
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .flex_shrink_0()
            .child(
                div()
                    .px(sp(1.5))
                    .py(sp(0.25))
                    .border_1()
                    .border_color(t.border_strong)
                    .rounded(radius::SMALL)
                    .typeset(Type::MICRO)
                    .text_color(t.dim)
                    .child(entry.decision.card_label()),
            )
            .into_any_element(),
    )
}

/// Under the inspector's context meter: what the last pass kept, and
/// its ledger.
fn context(
    _: &AtRun,
    view: &mut ViewCx<'_, FastCompactionUi>,
) -> Option<AnyElement> {
    let state = view.state?;
    state.last_pass()?;
    let kept = state.count(Decision::Keep) + state.count(Decision::Pinned);
    let t = view.theme().clone();
    let handle = view.handle.clone();
    Some(
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .typeset(Type::MICRO)
            .text_color(t.dim)
            .child(div().flex_1().child(format!("{kept} kept by {NAME}.")))
            .child(
                div()
                    .id("open-ledger-panel")
                    .child(link("Ledger", &t))
                    .on_click(move |_, _, cx| {
                        handle.navigate(ledger_link(), cx)
                    }),
            )
            .into_any_element(),
    )
}

/// One of its passes in the transcript: what it saved, and its ledger.
fn rewrite(
    at: &AtRewrite,
    view: &mut ViewCx<'_, FastCompactionUi>,
) -> Option<AnyElement> {
    let pass = view.state?.passes.get(&at.key)?.clone();
    let (before, after) = at.tokens.unwrap_or((pass.before, pass.after));
    let t = view.theme().clone();
    let compact = view.compact;
    let handle = view.handle.clone();
    let saved = before.saturating_sub(after);
    let ink = t.roles.plugin(NAME).unwrap_or(t.blue);
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(1.5))
            .px(sp(3.5))
            .py(sp(2.5))
            .rounded(radius::LARGE)
            .bg(t.card)
            .typeset(Type::CAPTION)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(sp(2.))
                    .child(icon(Icon::Plug, IconSize::COMPACT, ink))
                    .when(!compact, |row| {
                        row.child(div().text_color(ink).child(NAME))
                    })
                    .child(div().text_color(t.text_soft).child(format!(
                        "pruned {} tokens: {} to {}",
                        tokens(saved),
                        tokens(before),
                        tokens(after)
                    )))
                    .child(div().flex_1())
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "open-ledger-{}",
                                at.index
                            )))
                            .child(link("Ledger", &t))
                            .on_click(move |_, _, cx| {
                                handle.navigate(ledger_link(), cx)
                            }),
                    ),
            )
            .when(!compact, |card| {
                card.child(
                    mono(pass.detail.clone(), Type::MICRO, t.dim)
                        .pl(sp(5.))
                        .truncate(),
                )
            })
            .child(div().pl(sp(5.)).text_color(t.dim).child(
                "The next request resends the pruned transcript once, \
                     then turns are deltas again.",
            ))
            .into_any_element(),
    )
}

/// A run's context before and after the last pass, and what the pass
/// decided for each tool call.
fn ledger_page(view: &mut ViewCx<'_, FastCompactionUi>) -> AnyElement {
    let t = view.theme().clone();
    let compact = view.compact;
    let Some(run) = view.run.cloned() else {
        return ui::empty("That run is not in this workspace.", &t)
            .into_any_element();
    };
    let state = view.state.cloned().unwrap_or_default();
    let Some(pass) = state.last_pass().cloned() else {
        return ui::screen(
            "ledger",
            compact,
            ui::empty(
                format!(
                    "{NAME} has not pruned the context of {} yet.",
                    run.title
                ),
                &t,
            ),
        )
        .into_any_element();
    };
    let trigger = state.on.unwrap_or(false).then(trigger);
    ui::screen(
        "ledger",
        compact,
        div()
            .flex()
            .flex_col()
            .gap(sp(5.))
            .child(ui::screen_title(
                format!(
                    "Pruned {} tokens of {}'s tool history",
                    tokens(pass.before.saturating_sub(pass.after)),
                    run.title
                ),
                format!(
                    "{}. Decisions only escalate: keep, then drop the result, then drop the call. Forks inherit this ledger.",
                    pass.detail
                ),
                &t,
            ))
            .child(
                div()
                    .flex()
                    .when(compact, |layout| layout.flex_col())
                    .gap(sp(7.))
                    .child(meters(&run, &pass, trigger, &t))
                    .child(summary(&state, &t, compact)),
            )
            .child(heading("Ledger", &t))
            .child(table(&state, &t, compact)),
    )
    .into_any_element()
}

/// The context before and after the pass, on the window.
fn meters(run: &RunInfo, pass: &Pass, trigger: Option<f32>, t: &Theme) -> Div {
    let window = run.window.unwrap_or(pass.before.max(1));
    let meter = |label: &'static str, used: u64, fill| {
        div()
            .flex()
            .items_center()
            .gap(sp(3.))
            .child(
                div()
                    .w(rems(3.5))
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(label),
            )
            .child(
                div()
                    .flex_1()
                    .relative()
                    .child(bar(
                        used as f32 / window as f32,
                        14.,
                        fill,
                        t.raised,
                    ))
                    .children(trigger.map(|trigger| {
                        div()
                            .absolute()
                            .top(rems(-0.25))
                            .left(relative(trigger))
                            .w(rems(0.125))
                            .h(rems(1.375))
                            .bg(t.accent)
                    })),
            )
            .child(
                mono(tokens(used), Type::CAPTION, t.text)
                    .w(rems(3.))
                    .flex()
                    .justify_end(),
            )
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(meter("Before", pass.before, t.slate))
        .child(meter("After", pass.after, t.blue))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child(format!(
                    "Window {}.{}",
                    tokens(window),
                    trigger.map_or(String::new(), |trigger| format!(
                        " Pruning starts at {:.0}%.",
                        trigger * 100.
                    ))
                )),
        )
}

/// How many calls the pass made each decision for, and the outputs cut
/// as they arrived.
fn summary(state: &State, t: &Theme, compact: bool) -> Div {
    let value = |text: String| mono(text, Type::CAPTION, t.text);
    let count = |decision| value(state.count(decision).to_string());
    key_values(
        [
            ("plugin".into(), value(NAME.to_owned())),
            ("pinned".into(), count(Decision::Pinned)),
            ("kept".into(), count(Decision::Keep)),
            ("result dropped".into(), count(Decision::DropResult)),
            ("call dropped".into(), count(Decision::DropCall)),
            ("next request".into(), value("1 full resend".into())),
            (
                "outputs pruned".into(),
                value(match state.outputs {
                    0 => "none".to_owned(),
                    n => {
                        format!("{n} · −{} tokens", tokens(state.outputs_saved))
                    }
                }),
            ),
        ],
        t,
    )
    .p(sp(4.))
    .border_1()
    .border_color(t.border)
    .rounded(radius::BOX)
    .when(!compact, |card| card.w(rems(18.75)).flex_shrink_0())
}

/// The ledger: each call, with what the pass made of it.
fn table(state: &State, t: &Theme, compact: bool) -> Div {
    let rows = state.ledger.iter().map(|entry| {
        let (ink, fill) = match entry.decision {
            Decision::Pinned => (t.text, t.text),
            Decision::Keep => (t.text_soft, gpui::transparent_black()),
            Decision::DropResult => (t.blue, t.blue),
            Decision::DropCall => (t.dim, t.border_strong),
        };
        let odds =
            |p: Option<f32>| p.map_or("·".to_owned(), |p| format!("{p:.2}"));
        let decision = div()
            .flex()
            .items_center()
            .gap(sp(1.5))
            .typeset(Type::CAPTION)
            .text_color(ink)
            .child(
                div()
                    .size(rems(0.5))
                    .rounded(radius::HAIRLINE)
                    .bg(fill)
                    .border_1()
                    .border_color(ink),
            )
            .child(entry.decision.label());
        let input = mono(
            entry.input.clone(),
            Type::CAPTION,
            if entry.decision == Decision::DropCall {
                t.dim
            } else {
                t.text_soft
            },
        )
        .flex_1()
        .min_w(rems(0.))
        .truncate();
        let right = |text: String, width: f32, color| {
            mono(text, Type::CAPTION, color)
                .w(rems((width) / 16.))
                .flex()
                .justify_end()
        };
        div()
            .id(SharedString::from(format!("ledger-{}", entry.call_id)))
            .flex()
            .items_center()
            .gap(sp(3.))
            .px(sp(4.))
            .py(sp(2.))
            .border_b_1()
            .border_color(t.border)
            .when(!compact, |row| {
                row.child(
                    mono(format!("t{}", entry.turn), Type::CAPTION, t.dim)
                        .w(rems(2.5)),
                )
            })
            .child(
                mono(entry.tool.clone(), Type::CAPTION, t.blue)
                    .w(rems((if compact { 44. } else { 88. }) / 16.)),
            )
            .child(input)
            .when(!compact, |row| {
                row.child(right(tokens(entry.tokens), 64., t.muted))
                    .child(right(odds(entry.matters), 96., t.text))
                    .child(right(odds(entry.verbatim), 96., t.text))
            })
            .child(
                div()
                    .w(rems((if compact { 96. } else { 120. }) / 16.))
                    .child(decision),
            )
    });
    let head = |label: &str, width: f32, right: bool| {
        div()
            .w(rems((width) / 16.))
            .when(right, |cell| cell.flex().justify_end())
            .child(heading(label, t))
    };
    ui::card(t)
        .when(!compact, |card| {
            card.child(
                div()
                    .flex()
                    .gap(sp(3.))
                    .px(sp(4.))
                    .py(sp(2.25))
                    .bg(t.panel)
                    .border_b_1()
                    .border_color(t.border)
                    .child(head("Turn", 40., false))
                    .child(head("Tool", 88., false))
                    .child(div().flex_1().child(heading("Input", t)))
                    .child(head("Size", 64., true))
                    .child(head("Still matters", 96., true))
                    .child(head("Verbatim", 96., true))
                    .child(head("Decision", 120., false)),
            )
        })
        .children(rows)
        .when(state.ledger.is_empty(), |card| {
            card.child(ui::empty("The pass reported no per-call decisions.", t))
        })
}
