//! fast-compaction's UI (ADR 0017), as view trees (ADR 0045): what its
//! passes left of each tool call, on the call's card and on its ledger
//! page; each pass where it rewrote the transcript; where it steps in on
//! the context meter; and its line in the run's plugin list.

use std::{collections::BTreeMap, os::unix::fs::DirBuilderExt as _, sync::Arc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_jev::Jev;
use tau_ui_kit::format::{fine_usd, tokens};
use tau_ui_plugin::{
    Dropped,
    Fold,
    HostCx,
    HostHalf,
    Link,
    Manifest,
    OutputCut,
    Page,
    PluginInfo,
    RunCtx,
    RunCx,
    RunInfo,
    UiPlugin,
    ViewCx,
    points::{self, AtCard, AtRewrite, AtRun},
    tree::{KeyValue, Node, Tone},
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
            false => tau_jev::NO_KEY.to_owned(),
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

    fn name(&self) -> &'static str {
        NAME
    }

    fn rewrites_keep_transcript(&self) -> bool {
        true
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        // Pruning first: it is cheaper than a summary or a tree, and
        // they follow when pruning cannot help.
        tau_ui_plugin::Declaration::new()
            .before(&["tau-tree-compaction", "tau-luau-plugins"])
            .seams(&[
                tau_ui_plugin::Seam::Start,
                tau_ui_plugin::Seam::AfterTool,
                tau_ui_plugin::Seam::Rewrite,
            ])
            .services(&[tau_jev::SERVICE])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .page(
                Page::new("ledger", ledger_page)
                    .title(|_| "Context ledger".to_owned())
                    .framed(),
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

/// tau-fast-compaction on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct FastCompactionHost;

impl HostHalf for FastCompactionHost {
    type Plugin = FastCompactionUi;
    type Host = ();

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
            group: tau_ui_plugin::Group::Context,
            description: tau_jev::needs_jev(
                jev,
                "Prunes large command outputs as they arrive, and stale tool \
                 history",
            ),
            page: Some(ledger_link()),
            ..Default::default()
        }
    }
}

/// The plugin's mark: its plug.
fn plug() -> Node {
    Node::Icon {
        name: "plug".into(),
        tone: Tone::Info,
    }
}

/// What the last pass made of a call, on its card: what it changed.
fn badge(at: &AtCard, view: &mut ViewCx<'_, FastCompactionUi>) -> Option<Node> {
    let call_id = at.keys.first()?;
    let entry = view.state?.entry(call_id)?;
    // A result kept as it was needs no word: only what the pass changed
    // shows on the card.
    if matches!(entry.decision, Decision::Pinned | Decision::Keep) {
        return None;
    }
    Some(Node::Badge {
        text: entry.decision.card_label().into(),
        tone: Tone::Neutral,
    })
}

/// Under the inspector's context meter: what the last pass kept, and
/// its ledger.
fn context(_: &AtRun, view: &mut ViewCx<'_, FastCompactionUi>) -> Option<Node> {
    let state = view.state?;
    state.last_pass()?;
    let kept = state.count(Decision::Keep) + state.count(Decision::Pinned);
    Some(Node::row([
        Node::text(format!("{kept} kept by {NAME}.")),
        Node::Spacer,
        Node::Link {
            label: "Ledger".into(),
            to: ledger_link(),
        },
    ]))
}

/// One of its passes in the transcript: what it saved, and its ledger.
fn rewrite(
    at: &AtRewrite,
    view: &mut ViewCx<'_, FastCompactionUi>,
) -> Option<Node> {
    let pass = view.state?.passes.get(&at.key)?;
    let (before, after) = at.tokens.unwrap_or((pass.before, pass.after));
    let compact = view.compact;
    let saved = before.saturating_sub(after);
    let mut head = vec![plug()];
    if !compact {
        head.push(Node::Mono { text: NAME.into() });
    }
    head.extend([
        Node::text(format!(
            "pruned {} tokens: {} to {}",
            tokens(saved),
            tokens(before),
            tokens(after)
        )),
        Node::Spacer,
        Node::Link {
            label: "Ledger".into(),
            to: ledger_link(),
        },
    ]);
    let mut card = vec![Node::row(head)];
    if !compact {
        card.push(Node::Mono {
            text: pass.detail.clone(),
        });
    }
    card.push(Node::text(
        "The next request resends the pruned transcript once, then turns \
         are deltas again.",
    ));
    Some(Node::Card { children: card })
}

/// A run's context before and after the last pass, and what the pass
/// decided for each tool call.
fn ledger_page(view: &mut ViewCx<'_, FastCompactionUi>) -> Node {
    ledger(view.run, view.state, view.compact)
}

/// The ledger page's tree.
fn ledger(run: Option<&RunInfo>, state: Option<&State>, compact: bool) -> Node {
    let Some(run) = run else {
        return Node::Empty {
            text: "That run is not in this workspace.".into(),
        };
    };
    let state = state.cloned().unwrap_or_default();
    let Some(pass) = state.last_pass() else {
        return Node::Empty {
            text: format!(
                "{NAME} has not pruned the context of {} yet.",
                run.title
            ),
        };
    };
    let trigger = state.on.unwrap_or(false).then(trigger);
    Node::stack([
        Node::Title {
            text: format!(
                "Pruned {} tokens of {}'s tool history",
                tokens(pass.before.saturating_sub(pass.after)),
                run.title
            ),
            subtitle: Some(format!(
                "{}. Decisions only escalate: keep, then drop the result, \
                 then drop the call. Forks inherit this ledger.",
                pass.detail
            )),
        },
        Node::Card {
            children: vec![
                meters(run, pass, trigger),
                Node::Rule,
                summary(&state),
            ],
        },
        Node::Heading {
            text: "Ledger".into(),
        },
        table(&state, compact),
    ])
}

/// The context before and after the pass, on the window.
fn meters(run: &RunInfo, pass: &Pass, trigger: Option<f32>) -> Node {
    let window = run.window.unwrap_or(pass.before.max(1));
    let meter = |label: &str, used: u64| Node::Progress {
        share: used as f64 / window as f64,
        label: Some(format!("{label} · {}", tokens(used))),
    };
    Node::stack([
        meter("Before", pass.before),
        meter("After", pass.after),
        Node::text(format!(
            "Window {}.{}",
            tokens(window),
            trigger.map_or(String::new(), |trigger| format!(
                " Pruning starts at {:.0}%.",
                trigger * 100.
            ))
        )),
    ])
}

/// How many calls the pass made each decision for, and the outputs cut
/// as they arrived.
fn summary(state: &State) -> Node {
    let row = |key: &str, value: String| KeyValue {
        key: key.into(),
        value,
    };
    let count = |decision| state.count(decision).to_string();
    Node::Rows {
        rows: vec![
            row("plugin", NAME.to_owned()),
            row("pinned", count(Decision::Pinned)),
            row("kept", count(Decision::Keep)),
            row("result dropped", count(Decision::DropResult)),
            row("call dropped", count(Decision::DropCall)),
            row("next request", "1 full resend".into()),
            row(
                "outputs pruned",
                match state.outputs {
                    0 => "none".to_owned(),
                    n => {
                        format!("{n} · −{} tokens", tokens(state.outputs_saved))
                    }
                },
            ),
        ],
    }
}

/// The ledger: each call, with what the pass made of it. The phone
/// shows the tool, its input and the decision.
fn table(state: &State, compact: bool) -> Node {
    if state.ledger.is_empty() {
        return Node::Empty {
            text: "The pass reported no per-call decisions.".into(),
        };
    }
    let head = if compact {
        vec!["Tool", "Input", "Decision"]
    } else {
        vec![
            "Turn",
            "Tool",
            "Input",
            "Size",
            "Still matters",
            "Verbatim",
            "Decision",
        ]
    };
    let odds =
        |p: Option<f32>| p.map_or("·".to_owned(), |p| format!("{p:.2}"));
    let mono = |text: String| Node::Mono { text };
    let rows = state
        .ledger
        .iter()
        .map(|entry| {
            let tone = match entry.decision {
                Decision::Pinned | Decision::Keep => Tone::Neutral,
                Decision::DropResult => Tone::Info,
                Decision::DropCall => Tone::Warn,
            };
            let decision = Node::Badge {
                text: entry.decision.label().into(),
                tone,
            };
            let tool = mono(entry.tool.clone());
            let input = mono(entry.input.clone());
            if compact {
                vec![tool, input, decision]
            } else {
                vec![
                    mono(format!("t{}", entry.turn)),
                    tool,
                    input,
                    mono(tokens(entry.tokens)),
                    mono(odds(entry.matters)),
                    mono(odds(entry.verbatim)),
                    decision,
                ]
            }
        })
        .collect();
    // The input is what a row is about: it takes the room.
    let widths = if compact {
        vec![1., 2.5, 1.]
    } else {
        vec![0.5, 1.2, 3.5, 0.6, 0.9, 0.9, 1.1]
    };
    Node::Table {
        head: head.into_iter().map(str::to_owned).collect(),
        rows,
        widths,
    }
}
