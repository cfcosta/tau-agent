//! Runs a workload through the agent loop exactly as the app prunes: a
//! scripted model makes the workload's tool calls, fake tools answer
//! with its results, the fake `exec_command` cutting its output to the
//! tail as tau's does, and fast compaction, with only output pruning
//! at work, prunes the command's output before the model's next request.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

use async_trait::async_trait;
use futures_util::StreamExt as _;
use serde::Serialize;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    event::RunEvent,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_ai::message::{InputBlock, Message};
use tau_fast_compaction::{
    FastCompaction,
    NAME,
    OutputStats,
    Settings,
    output::{self, chunk_id},
    state,
};
use tau_jev::{Jev, JevError, Request, Response};
use tau_testing::scripted::ScriptedModel;
use tau_tools_host::truncate::{MAX_BYTES, MAX_LINES, truncate_tail};

use crate::{
    metrics::{Summary, Trial, summarize},
    workload::{Kind, Needle, Workload, generate},
};

/// Why a trial could not run to its end.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Store(#[from] tau_store::StoreError),
    #[error(transparent)]
    Agent(#[from] tau_agent::agent::AgentError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("the model never saw the command's result")]
    NoResult,
}

/// What tau's `exec_command` returns for `output`: the whole of it, or
/// its tail within 2,000 lines and 50 KB under a note saying so. The
/// second value says whether it cut the output.
pub fn exec_result(output: &str) -> (String, bool) {
    let tail = truncate_tail(output, MAX_LINES, MAX_BYTES);
    if tail.truncated() {
        (
            format!(
                "[output cut to the last {} of {} lines]\n{}",
                tail.output_lines, tail.total_lines, tail.content
            ),
            true,
        )
    } else {
        (output.to_owned(), false)
    }
}

/// How the fake `exec_command` returns an output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Exec {
    /// As tau's `exec_command` does: past 2,000 lines or 50 KB, the tail
    /// ([`exec_result`]).
    #[default]
    Tau,
    /// The whole output, as a shell without limits would.
    Whole,
}

/// Counts what a Jev was asked and what it cost.
pub struct Metered {
    inner: Arc<dyn Jev>,
    tally: Mutex<Tally>,
}

/// Requests, input tokens and dollars, Jev's answers by band, and each
/// question's largest answer by its id (a chunk's, such as `c7`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tally {
    pub requests: usize,
    pub input_tokens: u64,
    pub cost_usd: f64,
    pub bands: Bands,
    pub answers: std::collections::BTreeMap<String, f64>,
}

/// Noul answers at most this are confident noise.
pub const NOISE: f64 = 0.1;

/// Noul answers: at most [`NOISE`] (confident noise), above it and under
/// 0.5 (uncertain; with the default threshold, these go too), and 0.5 or
/// more (needed: the chunk stays).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, serde::Deserialize,
)]
pub struct Bands {
    pub noise: usize,
    pub uncertain: usize,
    pub needed: usize,
}

impl Bands {
    pub fn record(&mut self, noul: f64) {
        if noul <= NOISE {
            self.noise += 1;
        } else if noul < 0.5 {
            self.uncertain += 1;
        } else {
            self.needed += 1;
        }
    }
}

impl Metered {
    pub fn new(inner: Arc<dyn Jev>) -> Self {
        Self {
            inner,
            tally: Mutex::default(),
        }
    }

    pub fn tally(&self) -> Tally {
        self.tally.lock().expect("not poisoned").clone()
    }
}

#[async_trait]
impl Jev for Metered {
    async fn ask(&self, request: &Request) -> Result<Response, JevError> {
        let response = self.inner.ask(request).await;
        let mut tally = self.tally.lock().expect("not poisoned");
        tally.requests += 1;
        if let Ok(response) = &response {
            tally.input_tokens += response.usage.input_tokens;
            tally.cost_usd += response.usage().cost.total;
            for id in request.questions.keys() {
                if let Ok(noul) = response.noul(id) {
                    tally.bands.record(noul);
                    let most = tally.answers.entry(id.clone()).or_insert(noul);
                    *most = most.max(noul);
                }
            }
        }
        response
    }
}

/// A tool that answers each call from a list of arguments and results.
struct Scripted {
    name: String,
    answers: Vec<(Value, String)>,
    schema: Value,
}

#[async_trait]
impl AgentTool for Scripted {
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "Answers from the workload."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        self.answers
            .iter()
            .find(|(asked, _)| *asked == args)
            .map(|(_, result)| ToolOutput::text(result.clone()))
            .ok_or_else(|| {
                format!("the workload has no answer for {args}").into()
            })
    }
}

fn object_schema() -> Value {
    json!({"type": "object"})
}

/// Runs `workload` once, pruning with `jev` under `settings` (whose
/// `archive_dir` is replaced by `work`), with files in `work`,
/// `exec_command` returning the output as `exec` says.
pub async fn run(
    workload: &Workload,
    jev: Arc<dyn Jev>,
    mut settings: Settings,
    work: &Path,
    exec: Exec,
) -> Result<Trial, EvalError> {
    settings.archive_dir = work.to_owned();
    let chunk_lines = settings.output.chunk_lines;
    let (native, cut) = match exec {
        Exec::Tau => exec_result(&workload.output),
        Exec::Whole => (workload.output.clone(), false),
    };

    let mut model = ScriptedModel::new();
    for earlier in &workload.earlier {
        let (tool, args) = (earlier.tool.clone(), earlier.args.clone());
        model = model.turn(move |turn| turn.tool_call(tool, args));
    }
    let command = workload.command.clone();
    model = model
        .turn(move |turn| {
            turn.tool_call("exec_command", json!({"cmd": command}))
        })
        .turn(|turn| turn.text("done"));

    let metered = Arc::new(Metered::new(jev));
    let mut agent = Agent::new(model.clone()).tool(Scripted {
        name: "exec_command".into(),
        answers: vec![(json!({"cmd": workload.command}), native.clone())],
        schema: object_schema(),
    });
    let mut tools: Vec<Scripted> = Vec::new();
    for earlier in &workload.earlier {
        match tools.iter_mut().find(|tool| tool.name == earlier.tool) {
            Some(tool) => tool
                .answers
                .push((earlier.args.clone(), earlier.result.clone())),
            None => tools.push(Scripted {
                name: earlier.tool.clone(),
                answers: vec![(earlier.args.clone(), earlier.result.clone())],
                schema: object_schema(),
            }),
        }
    }
    for tool in tools {
        agent = agent.tool(tool);
    }
    let agent = agent.plugin(
        FastCompaction::shared(metered.clone() as Arc<dyn Jev>)
            .settings(settings),
    );

    let store = tau_store_sqlite::memory().await?;
    let started = Instant::now();
    let mut run = agent.start(workload.prompt.as_str(), &store);
    let events: Vec<RunEvent> = run.events().collect().await;
    run.outcome().await?;
    let latency_ms = started.elapsed().as_millis() as u64;

    let seen = model
        .requests()
        .last()
        .and_then(|request| match request.transcript.last() {
            Some(Message::ToolResult(result)) => Some(
                result
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        InputBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .ok_or(EvalError::NoResult)?;

    let mut stats: Option<OutputStats> = None;
    let mut error = None;
    for event in &events {
        match event {
            RunEvent::PluginReport { plugin, body, .. }
                if &**plugin == NAME && body["kind"] == "output" =>
            {
                stats = Some(serde_json::from_value(body.clone())?);
            }
            RunEvent::PluginError {
                plugin, message, ..
            } if &**plugin == NAME => error = Some(message.clone()),
            _ => {}
        }
    }

    let retained = |text: &str| {
        let lines: std::collections::HashSet<&str> = text.split('\n').collect();
        workload
            .needles
            .iter()
            .filter(|needle| lines.contains(needle.text.as_str()))
            .count()
    };
    let missed = {
        let lines: std::collections::HashSet<&str> = seen.split('\n').collect();
        workload
            .needles
            .iter()
            .filter(|needle| !lines.contains(needle.text.as_str()))
            .map(|needle| needle.text.clone())
            .collect()
    };
    let tally = metered.tally();
    let layout = layout(&native, &workload.needles, chunk_lines);
    let tokens_before = state::estimate_tokens(&native);
    let tokens_after = state::estimate_tokens(&seen);
    Ok(Trial {
        workload: workload.kind,
        seed: workload.seed,
        needles: workload.needles.len(),
        retained: retained(&seen),
        tail_retained: retained(&native),
        missed,
        lines: workload.output.split('\n').count(),
        kept_lines: stats
            .as_ref()
            .filter(|stats| stats.pruned)
            .map(|stats| stats.lines - stats.dropped_lines),
        chunks: stats.as_ref().map(|stats| stats.chunks),
        kept_chunks: stats.as_ref().map(|stats| stats.kept),
        cut,
        tokens_full: state::estimate_tokens(&workload.output),
        tokens_before,
        tokens_after,
        replaced: seen != native,
        requests: tally.requests,
        jev_input_tokens: tally.input_tokens,
        cost_usd: tally.cost_usd,
        answers: tally.bands,
        scores: (0..layout.chunk_tokens.len())
            .map(|index| tally.answers.get(&chunk_id(index)).copied())
            .collect(),
        chunk_tokens: layout.chunk_tokens,
        needle_chunks: layout.needle_chunks,
        latency_ms,
        archive: stats.and_then(|stats| stats.archive),
        error,
    })
}

/// How the plugin chunks the output the tool returned; a needle the
/// tool cut away is in no chunk.
struct Layout {
    /// Each chunk's estimated tokens.
    chunk_tokens: Vec<usize>,
    /// The chunk each needle is in, in the workload's order.
    needle_chunks: Vec<usize>,
}

fn layout(output: &str, needles: &[Needle], chunk_lines: usize) -> Layout {
    let lines = output::lines(output);
    let ranges = output::chunks(lines.len(), chunk_lines);
    Layout {
        chunk_tokens: ranges
            .iter()
            .map(|range| {
                state::estimate_tokens(&output::text_of(&lines, range.clone()))
            })
            .collect(),
        needle_chunks: needles
            .iter()
            .filter_map(|needle| {
                let piece =
                    lines.iter().position(|line| line.text == needle.text)?;
                ranges.iter().position(|range| range.contains(&piece))
            })
            .collect(),
    }
}

/// What to evaluate.
#[derive(Debug, Clone)]
pub struct Config {
    pub kinds: Vec<Kind>,
    pub seeds: Vec<u64>,
    /// Stop once Jev spend passes this many dollars.
    pub budget_usd: Option<f64>,
    pub settings: Settings,
    pub exec: Exec,
    /// Where each trial's files go, in a directory of its own.
    pub work: PathBuf,
}

impl Config {
    pub fn new(work: PathBuf) -> Self {
        Self {
            kinds: Kind::ALL.to_vec(),
            seeds: vec![0],
            budget_usd: None,
            settings: Settings::default(),
            exec: Exec::Tau,
            work,
        }
    }
}

/// Every trial, the summaries, and what they cost.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub trials: Vec<Trial>,
    pub summaries: Vec<Summary>,
    pub spent_usd: f64,
    /// Why the evaluation stopped before its last trial.
    pub stopped: Option<String>,
}

/// Runs every kind for every seed, seed by seed, so a budget that runs
/// out leaves every kind about as sampled. `progress` sees each trial.
pub async fn evaluate(
    config: &Config,
    jev: Arc<dyn Jev>,
    progress: &mut dyn FnMut(&Trial),
) -> Result<Report, EvalError> {
    let mut trials = Vec::new();
    let mut spent = 0.0;
    let mut stopped = None;
    'seeds: for &seed in &config.seeds {
        for &kind in &config.kinds {
            if let Some(budget) = config.budget_usd
                && spent >= budget
            {
                stopped = Some(format!(
                    "spent ${spent:.4}, past the ${budget:.2} budget"
                ));
                break 'seeds;
            }
            let workload = generate(kind, seed);
            let dir = config.work.join(format!("{}-{seed}", kind.name()));
            std::fs::create_dir_all(&dir)?;
            let trial = run(
                &workload,
                jev.clone(),
                config.settings.clone(),
                &dir,
                config.exec,
            )
            .await?;
            spent += trial.cost_usd;
            progress(&trial);
            trials.push(trial);
        }
    }
    Ok(Report {
        summaries: summarize(&trials),
        trials,
        spent_usd: spent,
        stopped,
    })
}
