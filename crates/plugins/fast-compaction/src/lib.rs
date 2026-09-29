//! Keeps a run's context lean with Jev, in two stages
//! (`docs/reference/fast-compaction.md`):
//!
//! - **Output pruning,** at `after_tool`: a large `bash` result is
//!   trimmed to the chunks of lines Jev says the task still needs,
//!   before the model first sees it, with the whole output archived to a
//!   file the model can read. Nothing already sent changes, so it costs
//!   no resend. After `tamaratran/jev-pruner`.
//! - **History pruning,** between turns: stale tool calls and results
//!   are dropped or cut, as a context rewrite. A port of
//!   `joelhooks/pi-fast-jev-compaction`, itself built on
//!   `tamaratran/fast-jev-compaction` (see `THIRD_PARTY_NOTICES.md`),
//!   onto tau's context seam (`docs/reference/plugins.md`).
//!
//! History pruning:
//!
//! - Between turns, once the context passes a share of the window and has
//!   grown by a cooldown since the last pass, and whenever the context
//!   overflows, it asks Jev two yes/no questions per tool call that is
//!   not pinned: does knowing the call was made still matter, and does
//!   its full result still need to stay verbatim. Jev never sees the
//!   results, only the tools, their inputs, and the results' sizes; a
//!   history too large for one state is split into segments asked about
//!   separately, never abridged.
//! - Each call is kept, has its result cut to a head and a note naming
//!   an archive of it, or goes with its result. Decisions only ever
//!   escalate, and are kept in a ledger that a fork of the run inherits.
//! - When the pass saves at least `min_reduction_ratio` of the context,
//!   the pruned transcript replaces the run's, as a context rewrite:
//!   one full resend, then deltas again. Otherwise it declines, and the
//!   next context plugin (summarizing compaction, added after it) gets
//!   the chance.
//!
//! Deviations from pi: pi re-applies its ledger to a per-turn view and
//! applies even a small saving; tau stores the rewrite once, and only
//! rewrites for a saving worth a full resend. The first and the last
//! `preserve_recent` messages are pinned, and at least the last one
//! always is, since the loop requires a rewrite to keep it.

pub mod archive;
pub mod decide;
pub mod history;
pub mod ledger;
pub mod output;
pub mod plan;
pub mod state;

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tau_agent::{
    plugin::{
        ContextView,
        Plugin,
        PluginCtx,
        PluginRun,
        Rewrite,
        RunPlan,
        ToolResultView,
        Trigger,
    },
    tool::ToolOutput,
};
use tau_ai::message::{InputBlock, Message, TextContent};
use tau_jev::Jev;

pub use crate::{
    decide::{Action, Decision},
    ledger::Ledger,
    output::OutputStats,
};

/// The name the plugin goes by in events and stored rewrites.
pub const NAME: &str = "fast-compaction";

/// Settings, pi's defaults for history pruning. See the crate docs for
/// what each governs.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// What the run is for, as context for Jev. Without one, the user's
    /// last three prompts.
    pub goal: Option<String>,
    /// A Noul at or above this keeps what it asks about. 0.5.
    pub keep_threshold: f64,
    /// Messages at the end that are never pruned. 6; at least 1 applies.
    pub preserve_recent: usize,
    /// The largest state sent to Jev, in estimated tokens. 25,000.
    pub max_state_tokens: usize,
    /// The largest request, state and questions, in estimated tokens.
    /// 30,000.
    pub max_request_tokens: usize,
    /// Characters of a dropped result that stay. 300.
    pub head_chars: usize,
    /// Share of the context window, in percent, past which a pass runs
    /// between turns. 60.
    pub compact_at_percent: f64,
    /// The smallest share of the context, as JSON, a pass must save to
    /// rewrite it. 0.25.
    pub min_reduction_ratio: f64,
    /// Tokens the context must grow by after a pass before another runs
    /// between turns. 8,000.
    pub cooldown_tokens: u64,
    /// Overrides the model's context window, for a model the registry
    /// does not know. Without either, only an overflow runs a pass.
    pub context_window: Option<u64>,
    /// Where archives of pruned outputs and cut results go, as files only
    /// their owner can read. The system's temporary directory.
    pub archive_dir: PathBuf,
    /// Output pruning, at `after_tool`.
    pub output: OutputPruning,
}

/// Settings of output pruning, jev-pruner's defaults unless said.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputPruning {
    /// On by default.
    pub enabled: bool,
    /// Outputs estimated at this many tokens or fewer pass untouched,
    /// without a Jev call. 10,000.
    pub min_output_tokens: usize,
    /// Lines per chunk, before chunks merge to stay under 200. 20.
    pub chunk_lines: usize,
    /// A Noul at or above this, against any segment, keeps its chunk.
    /// 0.5.
    pub keep_threshold: f64,
    /// The largest state sent to Jev, in estimated tokens. 25,000.
    pub max_state_tokens: usize,
    /// The largest request, state and questions, in estimated tokens.
    /// 30,000.
    pub max_request_tokens: usize,
    /// Jev requests per output, at most. 12.
    pub max_output_requests: usize,
    /// The smallest share of the whole output's estimated tokens the
    /// pruned output must save to replace the result, which it must not
    /// outgrow either. 0.1 (jev-pruner has none).
    pub min_reduction_ratio: f64,
}

impl Default for OutputPruning {
    fn default() -> Self {
        Self {
            enabled: true,
            min_output_tokens: 10_000,
            chunk_lines: 20,
            keep_threshold: 0.5,
            max_state_tokens: 25_000,
            max_request_tokens: 30_000,
            max_output_requests: 12,
            min_reduction_ratio: 0.1,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            goal: None,
            keep_threshold: 0.5,
            preserve_recent: 6,
            max_state_tokens: 25_000,
            max_request_tokens: 30_000,
            head_chars: 300,
            compact_at_percent: 60.0,
            min_reduction_ratio: 0.25,
            cooldown_tokens: 8_000,
            context_window: None,
            archive_dir: std::env::temp_dir(),
            output: OutputPruning::default(),
        }
    }
}

/// The plugin. Add it before summarizing compaction.
#[derive(Clone)]
pub struct FastCompaction {
    jev: Arc<dyn Jev>,
    settings: Settings,
}

impl std::fmt::Debug for FastCompaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FastCompaction")
            .field("settings", &self.settings)
            .finish_non_exhaustive()
    }
}

impl FastCompaction {
    /// Prunes with `jev`, such as `tau_jev::TypeSafe::from_env()?`.
    pub fn new(jev: impl Jev) -> Self {
        Self {
            jev: Arc::new(jev),
            settings: Settings::default(),
        }
    }

    /// Prunes with a Jev shared with other plugins.
    pub fn shared(jev: Arc<dyn Jev>) -> Self {
        Self {
            jev,
            settings: Settings::default(),
        }
    }

    pub fn settings(mut self, settings: Settings) -> Self {
        self.settings = settings;
        self
    }
}

/// What a rewrite stores: the ledger, and how the pass went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Details {
    pub decisions: Vec<Decision>,
    pub stats: Stats,
}

/// How one pass went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    /// Tool calls with results in the transcript.
    pub calls: usize,
    pub pinned: usize,
    pub kept: usize,
    pub results_dropped: usize,
    pub calls_dropped: usize,
    pub requests: usize,
    /// The largest state's estimated tokens, and how the history went
    /// into states: `whole`, or `N segments`.
    pub state_tokens: usize,
    pub state_stage: String,
    pub chars_before: usize,
    pub chars_after: usize,
    pub reduction_ratio: f64,
}

#[async_trait]
impl Plugin for FastCompaction {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> anyhow::Result<Box<dyn PluginRun>> {
        let ledger = match plan.last_rewrite() {
            Some(details) => {
                let details: Details = serde_json::from_value(details.clone())?;
                Ledger::from_decisions(details.decisions)
            }
            None => Ledger::default(),
        };
        let window = self.settings.context_window.or_else(|| {
            tau_ai::model::find(plan.model()).map(|model| model.context_window)
        });
        Ok(Box::new(FastCompactionRun {
            jev: self.jev.clone(),
            settings: self.settings.clone(),
            window,
            ledger,
            last_pass: None,
        }))
    }
}

struct FastCompactionRun {
    jev: Arc<dyn Jev>,
    settings: Settings,
    window: Option<u64>,
    ledger: Ledger,
    /// The context size when the last pass that asked Jev started, for
    /// the cooldown.
    last_pass: Option<u64>,
}

#[async_trait]
impl PluginRun for FastCompactionRun {
    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Option<Rewrite>> {
        if view.trigger == Trigger::TurnEnd && !self.due(view.tokens) {
            return Ok(None);
        }
        tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => anyhow::bail!("the pass was cancelled"),
            rewrite = self.pass(view, ctx) => rewrite,
        }
    }

    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        output: &mut ToolOutput,
        ctx: &PluginCtx,
    ) -> anyhow::Result<()> {
        let settings = &self.settings.output;
        if !settings.enabled {
            return Ok(());
        }
        let Some(gated) = output::gate(
            &view.call.name,
            view.is_error,
            &output.content,
            settings.min_output_tokens,
        ) else {
            return Ok(());
        };
        let pruned = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => anyhow::bail!("output pruning was cancelled"),
            pruned = prune_output(&*self.jev, &self.settings, view, gated, ctx) => pruned?,
        };
        if let Some(text) = pruned {
            output.content = vec![InputBlock::Text(TextContent {
                text,
                text_signature: None,
            })];
        }
        Ok(())
    }
}

/// Prunes one output that passed the gate: plans the requests, asks Jev,
/// renders what stays, and writes the archive when `bash` did not spill
/// the output already. The pruned text, when it saves at least
/// `min_reduction_ratio` of the whole output and is no larger than what
/// the model would otherwise see. Reports how it went whenever Jev was
/// asked, and records it with the run.
async fn prune_output(
    jev: &dyn Jev,
    settings: &Settings,
    view: &ToolResultView<'_>,
    gated: output::Gated,
    ctx: &PluginCtx,
) -> anyhow::Result<Option<String>> {
    let pruning = &settings.output;
    let mut transcript = state::entries(view.transcript);
    transcript
        .extend(state::entries(&[Message::Assistant(view.message.clone())]));
    let task = state::goal_or_prompts(settings.goal.as_deref(), &transcript);
    let records = output::output_records(&transcript);
    let command = view.call.args["command"].as_str().unwrap_or_default();
    let lines = output::lines(&gated.full);
    let Some(planned) =
        output::plan_requests(&lines, &records, &task, command, pruning)
            .map_err(anyhow::Error::msg)?
    else {
        return Ok(None);
    };
    if planned.requests.is_empty() {
        return Ok(None);
    }
    let asked = output::ask(jev, &planned, pruning.keep_threshold, |usage| {
        ctx.charge(usage)
    })
    .await?;
    let archive = gated
        .spill
        .clone()
        .unwrap_or_else(|| archive::new_path(&settings.archive_dir, "output"));
    let text = output::render(
        &lines,
        &planned.chunks,
        &asked.keep,
        &archive.display().to_string(),
    );
    let tokens_before = state::estimate_tokens(&gated.seen);
    let tokens_after = state::estimate_tokens(&text);
    let dropped_lines = planned
        .chunks
        .iter()
        .zip(&asked.keep)
        .filter(|(_, kept)| !**kept)
        .map(|(range, _)| range.len())
        .sum::<usize>();
    // Measured against the whole output, which the pruned one is cut
    // from, and never larger than what the model would otherwise see:
    // bash's tail when it truncated.
    let tokens_full = state::estimate_tokens(&gated.full);
    let pruned = dropped_lines > 0
        && tokens_after <= tokens_before
        && tokens_after as f64
            <= tokens_full as f64 * (1.0 - pruning.min_reduction_ratio);
    if pruned && gated.spill.is_none() {
        archive::write(&archive, &gated.full).map_err(|error| {
            anyhow::anyhow!("cannot archive to {}: {error}", archive.display())
        })?;
    }
    let stats = OutputStats {
        call_id: view.call.id.clone(),
        lines: lines.len(),
        chunks: planned.chunks.len(),
        kept: asked.keep.iter().filter(|kept| **kept).count(),
        dropped_lines,
        segments: planned.segments,
        requests: asked.requests,
        tokens_before,
        tokens_after: if pruned { tokens_after } else { tokens_before },
        pruned,
        archive: pruned.then(|| archive.display().to_string()),
    };
    let mut report = serde_json::to_value(&stats).expect("stats serialize");
    report["kind"] = "output".into();
    ctx.report(report.clone());
    // History reads it back; a failed write loses only what the card
    // says.
    let _ = ctx.record(&report).await;
    Ok(pruned.then_some(text))
}

impl FastCompactionRun {
    /// Whether a pass is due between turns: past the share of the window,
    /// and past the cooldown since the last pass.
    fn due(&self, tokens: u64) -> bool {
        let Some(window) = self.window.filter(|window| *window > 0) else {
            return false;
        };
        let percent = tokens as f64 * 100.0 / window as f64;
        let cooled = self.last_pass.is_none_or(|last| {
            tokens.saturating_sub(last) >= self.settings.cooldown_tokens
        });
        percent >= self.settings.compact_at_percent && cooled
    }

    async fn pass(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> anyhow::Result<Option<Rewrite>> {
        let settings = &self.settings;
        let preserve = settings.preserve_recent.max(1);
        let entries = state::entries(view.transcript);
        let calls = state::collect_calls(&entries, preserve);
        if calls.iter().all(|call| call.pinned) {
            return Ok(None);
        }
        // Unlike pi, the cooldown starts only with a pass that asks Jev:
        // it exists to space out paid passes and full resends, and a pass
        // with nothing to ask costs neither.
        self.last_pass = Some(view.tokens);
        let decided =
            decide::decide(&*self.jev, &entries, &calls, settings, |usage| {
                ctx.charge(usage)
            })
            .await?;

        let mut staged = self.ledger.clone();
        staged.merge(decided.decisions.iter().cloned());
        let archives = staged.assign_archives(
            view.transcript,
            settings.head_chars,
            &settings.archive_dir,
        );
        let messages = staged.apply(view.transcript, settings.head_chars);
        let chars_before = ledger::measure(view.transcript);
        let chars_after = ledger::measure(&messages);
        let reduction_ratio = if chars_before == 0 {
            0.0
        } else {
            chars_before.saturating_sub(chars_after) as f64
                / chars_before as f64
        };
        if reduction_ratio < settings.min_reduction_ratio {
            return Ok(None);
        }
        // Archives are written only for a rewrite that happens; one that
        // cannot be written fails the pass rather than cut a result the
        // model could not read back.
        for (path, text) in &archives {
            archive::write(path, text).map_err(|error| {
                anyhow::anyhow!("cannot archive to {}: {error}", path.display())
            })?;
        }
        let count = |action| {
            decided
                .decisions
                .iter()
                .zip(&calls)
                .filter(|(decision, call)| {
                    !call.pinned && decision.action == action
                })
                .count()
        };
        let stats = Stats {
            calls: calls.len(),
            pinned: calls.iter().filter(|call| call.pinned).count(),
            kept: count(Action::Keep),
            results_dropped: count(Action::DropResult),
            calls_dropped: count(Action::DropCall),
            requests: decided.requests,
            state_tokens: decided.state_tokens,
            state_stage: if decided.segments == 1 {
                "whole".to_owned()
            } else {
                format!("{} segments", decided.segments)
            },
            chars_before,
            chars_after,
            reduction_ratio,
        };
        self.ledger = staged;
        let details = Details {
            decisions: self.ledger.decisions().cloned().collect(),
            stats,
        };
        // Interfaces show the ledger; the rewrite stores it for forks.
        let mut report =
            serde_json::to_value(&details).expect("details serialize");
        report["kind"] = "ledger".into();
        ctx.report(report);
        Ok(Some(Rewrite {
            messages,
            details: serde_json::to_value(details).expect("details serialize"),
        }))
    }
}
