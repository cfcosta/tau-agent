//! Runs the trials: for each scenario, variant, arm and repetition, a
//! fresh repository, the first run, the change between, the second run
//! and its check, until done or the budget is spent.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Context as _;
use futures_util::StreamExt;
use regex::Regex;
use serde::Serialize;
use tau_agent::limits::Limits;
use tau_ai::llm::Llm;
use tau_memory::{MemoryPlugin, plugin::START_HITS};
use tau_store::Store;

use crate::{
    arm::{self, Arm, IndexFactory, MAX_TURNS, RunSetup, Transcript},
    metrics::{self, Meter, RunMetrics, Trial},
    scenario::{Scenario, Variant},
};

/// Which run of a trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    First,
    Second,
}

/// Names one run: what gives it its model.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RunKey {
    pub scenario: &'static str,
    pub variant: Variant,
    pub arm: Arm,
    pub trial: u32,
    pub stage: Stage,
}

/// What to run.
pub struct Config {
    pub model: String,
    pub scenarios: Vec<&'static Scenario>,
    pub variants: Vec<Variant>,
    pub arms: Vec<Arm>,
    pub trials: u32,
    pub budget: Budget,
    /// Where trials make their repositories, each in a directory of its
    /// own, removed when it ends.
    pub work: PathBuf,
    /// What memory and transcript search index with.
    pub index: IndexFactory,
    /// Its name, for the report.
    pub index_name: String,
    /// The most turns a run may take.
    pub max_turns: u32,
    /// The longest a run may take.
    pub timeout: Duration,
}

impl Config {
    /// Every scenario, variant and arm, once, BM25 for search.
    pub fn new(model: impl Into<String>, work: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            scenarios: crate::scenario::SCENARIOS.iter().collect(),
            variants: Variant::ALL.to_vec(),
            arms: Arm::ALL.to_vec(),
            trials: 1,
            budget: Budget::default(),
            work: work.into(),
            index: arm::keyword_index(),
            index_name: "bm25".into(),
            max_turns: MAX_TURNS,
            timeout: Duration::from_secs(15 * 60),
        }
    }

    /// How many trials the configuration asks for.
    pub fn planned(&self) -> usize {
        self.scenarios.len()
            * self.variants.len()
            * self.arms.len()
            * self.trials as usize
    }
}

/// Money a whole evaluation may spend, in USD.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Budget {
    pub limit: Option<f64>,
    pub spent: f64,
}

impl Budget {
    pub fn new(limit: Option<f64>) -> Self {
        Self { limit, spent: 0.0 }
    }

    pub fn charge(&mut self, usd: f64) {
        self.spent += usd;
    }

    /// What is left, when there is a limit.
    pub fn remaining(&self) -> Option<f64> {
        self.limit.map(|limit| (limit - self.spent).max(0.0))
    }

    pub fn exhausted(&self) -> bool {
        self.remaining().is_some_and(|left| left <= 0.0)
    }
}

/// Everything an evaluation did.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub model: String,
    pub index: String,
    pub trials: Vec<Trial>,
    pub summaries: Vec<metrics::Summary>,
    pub spent_usd: f64,
    pub budget_usd: Option<f64>,
    /// Why it stopped before every trial ran, if it did.
    pub stopped: Option<String>,
}

/// Runs every trial `config` asks for, in order, with the model
/// `models` gives each run, calling `done` after each trial. Stops early
/// when the budget is spent: a trial whose first run spent it is not
/// finished.
pub async fn evaluate(
    config: &Config,
    models: &(dyn Fn(&RunKey) -> Arc<dyn Llm> + Sync),
    done: &mut (dyn FnMut(&Trial) + Send),
) -> anyhow::Result<Report> {
    let mut budget = config.budget.clone();
    let mut trials = Vec::new();
    let mut stopped = None;
    'all: for scenario in &config.scenarios {
        for &variant in &config.variants {
            for &arm in &config.arms {
                for trial in 0..config.trials {
                    let key = |stage| RunKey {
                        scenario: scenario.name,
                        variant,
                        arm,
                        trial,
                        stage,
                    };
                    if budget.exhausted() {
                        stopped = Some(spent(&budget));
                        break 'all;
                    }
                    let run = run_trial(
                        config,
                        scenario,
                        key(Stage::First),
                        &|stage| models(&key(stage)),
                        &mut budget,
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "{} {} {} #{trial}",
                            scenario.name,
                            variant.name(),
                            arm.name()
                        )
                    })?;
                    match run {
                        Some(result) => {
                            done(&result);
                            trials.push(result);
                        }
                        None => {
                            stopped = Some(spent(&budget));
                            break 'all;
                        }
                    }
                }
            }
        }
    }
    Ok(Report {
        model: config.model.clone(),
        index: config.index_name.clone(),
        summaries: metrics::summarize(&trials),
        trials,
        spent_usd: budget.spent,
        budget_usd: budget.limit,
        stopped,
    })
}

fn spent(budget: &Budget) -> String {
    format!(
        "the budget of ${:.2} is spent (${:.4})",
        budget.limit.unwrap_or_default(),
        budget.spent
    )
}

/// One trial in a directory of its own. `None` when the budget ran out
/// after the first run.
async fn run_trial(
    config: &Config,
    scenario: &'static Scenario,
    key: RunKey,
    models: &(dyn Fn(Stage) -> Arc<dyn Llm> + Sync),
    budget: &mut Budget,
) -> anyhow::Result<Option<Trial>> {
    let dir = tempfile::Builder::new()
        .prefix(&format!("{}-{}-", scenario.name, key.arm.name()))
        .tempdir_in(&config.work)?;
    let repo = dir.path().join("repo");
    scenario.setup(&repo)?;
    let memory = if key.arm.uses_memory() {
        Some(arm::memory_plugin(
            &dir.path().join("memory"),
            &config.index,
        )?)
    } else {
        None
    };
    let store = Store::memory().await?;

    // The first run.
    let transcript = Transcript::default();
    let (mut first, _) = run_once(
        RunSetup {
            llm: models(Stage::First),
            model: &config.model,
            repo: &repo,
            arm: key.arm,
            memory: memory.as_ref(),
            context: None,
            limits: limits(config, budget),
            transcript: transcript.clone(),
        },
        scenario.first.prompt,
        &store,
    )
    .await?;
    budget.charge(first.cost_usd);
    first.success =
        Some(on_blocking(&repo, move |repo| scenario.first_done(repo)).await?);
    let transcript = transcript.lock().expect("not poisoned").clone();
    let memory_saved = match key.arm {
        Arm::None => false,
        Arm::MemoryMd => arm::memory_md_context(&repo).is_some(),
        Arm::Transcripts => !transcript.is_empty(),
        Arm::Memory | Arm::MemoryConsolidate => {
            let plugin = memory.as_ref().expect("a memory arm");
            !plugin
                .scopes()
                .repo
                .lock()
                .expect("not poisoned")
                .notes()
                .is_empty()
        }
    };
    if budget.exhausted() {
        return Ok(None);
    }

    // Between the runs: a fresh checkout, and maybe the change.
    let variant = key.variant;
    on_blocking(&repo, move |repo| scenario.between(repo, variant)).await?;
    if variant == Variant::Changed
        && let Some(plugin) = &memory
    {
        mark_changed(plugin, scenario.change.paths)?;
    }

    // The second run, with what the arm carries over.
    let prompt = scenario.second.prompt;
    let context = match key.arm {
        Arm::None | Arm::Memory | Arm::MemoryConsolidate => None,
        Arm::MemoryMd => arm::memory_md_context(&repo),
        Arm::Transcripts => arm::transcript_context(
            &arm::chunks(&transcript),
            prompt,
            (config.index)(&dir.path().join("transcript-embeddings")),
        )?,
    };
    let memory_given = match &memory {
        Some(plugin) => starts_with_memory(plugin, prompt)?,
        None => context.is_some(),
    };
    let (mut second, calls) = run_once(
        RunSetup {
            llm: models(Stage::Second),
            model: &config.model,
            repo: &repo,
            arm: key.arm,
            memory: memory.as_ref(),
            context,
            limits: limits(config, budget),
            transcript: Transcript::default(),
        },
        prompt,
        &store,
    )
    .await?;
    budget.charge(second.cost_usd);
    second.success =
        Some(on_blocking(&repo, move |repo| scenario.second_done(repo)).await?);
    let stale_used = match variant {
        Variant::Stable => None,
        Variant::Changed => {
            let stale = Regex::new(scenario.change.stale)?;
            Some(metrics::stale_used(&calls, &stale))
        }
    };
    Ok(Some(Trial {
        scenario: scenario.name.to_owned(),
        variant,
        arm: key.arm,
        trial: key.trial,
        first,
        second,
        memory_saved,
        memory_given,
        memory_calls: metrics::memory_calls(&calls),
        stale_used,
        calls,
    }))
}

/// What a commit changing `paths` does to memory in the app
/// (tau-ui's `stale_on_commit`): the notes about them, written before
/// now, are marked as possibly stale.
pub fn mark_changed(
    plugin: &MemoryPlugin,
    paths: &[&str],
) -> anyhow::Result<Vec<String>> {
    let paths: Vec<String> =
        paths.iter().map(|path| (*path).to_owned()).collect();
    plugin.mark_stale(&paths, "commit between runs changed it", plugin.now())
}

/// Whether a run on `task` starts with memory in its context: the index
/// note, or a hit for the task.
fn starts_with_memory(
    plugin: &MemoryPlugin,
    task: &str,
) -> anyhow::Result<bool> {
    let memory = plugin.scopes().repo.lock().expect("not poisoned");
    Ok(memory.index_note().is_some()
        || !memory.search(task, START_HITS)?.is_empty())
}

/// A run's limits: its turns and time, and what is left of the budget.
fn limits(config: &Config, budget: &Budget) -> Limits {
    let limits = Limits::default()
        .max_turns(config.max_turns)
        .timeout(config.timeout);
    match budget.remaining() {
        Some(left) => limits.max_usd(left),
        None => limits,
    }
}

/// Runs the agent on `prompt`; returns its figures, unchecked, and its
/// tool calls.
async fn run_once(
    setup: RunSetup<'_>,
    prompt: &str,
    store: &Store,
) -> anyhow::Result<(RunMetrics, Vec<metrics::Call>)> {
    let agent = arm::agent(setup);
    let started = Instant::now();
    let mut meter = Meter::new();
    let mut run = agent.start(prompt, store);
    {
        let mut events = std::pin::pin!(run.events());
        while let Some(event) = events.next().await {
            meter.observe(&event);
        }
    }
    let outcome = run.outcome().await;
    let metrics = meter.finish(&outcome, started.elapsed());
    Ok((metrics, meter.calls()))
}

/// Runs `f` on the repository off the async workers: scripts block.
async fn on_blocking<T: Send + 'static>(
    repo: &Path,
    f: impl FnOnce(&Path) -> anyhow::Result<T> + Send + 'static,
) -> anyhow::Result<T> {
    let repo = repo.to_owned();
    tokio::task::spawn_blocking(move || f(&repo))
        .await
        .context("the check stopped")?
}
