//! Runs the trials against a real model.

use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde::Serialize;
use tau_agent::limits::Limits;
use tau_ai::llm::Llm;

use crate::{
    EvalError,
    arm::{self, Arm},
    metrics::{self, Meter, Trial},
    task::{self, Kind},
};

/// What to run.
pub struct Config {
    pub model: String,
    pub kinds: Vec<Kind>,
    /// Lines of the input, each size a trial.
    pub sizes: Vec<usize>,
    pub arms: Vec<Arm>,
    pub trials: u32,
    pub budget_usd: Option<f64>,
    pub work: PathBuf,
    /// The documents the `docs` task reads.
    pub docs: PathBuf,
    pub max_turns: u32,
    pub timeout: Duration,
}

impl Config {
    pub fn new(model: impl Into<String>, work: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            kinds: Kind::ALL.to_vec(),
            sizes: vec![1_000, 6_000],
            arms: Arm::ALL.to_vec(),
            trials: 1,
            budget_usd: None,
            work: work.into(),
            docs: PathBuf::from("docs"),
            max_turns: 40,
            timeout: Duration::from_secs(20 * 60),
        }
    }

    pub fn planned(&self) -> usize {
        self.kinds.len()
            * self.sizes.len()
            * self.arms.len()
            * self.trials as usize
    }
}

/// Everything an evaluation did.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub model: String,
    pub trials: Vec<Trial>,
    pub summaries: Vec<metrics::Summary>,
    pub spent_usd: f64,
    pub stopped: Option<String>,
}

/// Runs every trial, calling `done` after each; stops when the budget is
/// spent.
pub async fn evaluate(
    config: &Config,
    llm: Arc<dyn Llm>,
    done: &mut (dyn FnMut(&Trial) + Send),
) -> Result<Report, EvalError> {
    let store = tau_store_sqlite::memory().await?;
    let mut trials = Vec::new();
    let mut spent = 0.0;
    let mut stopped = None;
    'all: for &kind in &config.kinds {
        for &lines in &config.sizes {
            for trial in 0..config.trials {
                // The same input for every arm of a trial.
                let seed = 1_000 * u64::from(trial) + lines as u64;
                let dir = tempfile::Builder::new()
                    .prefix("rlm-")
                    .tempdir_in(&config.work)?;
                let instance = if kind == Kind::Docs {
                    task::generate_docs(&config.docs, seed, dir.path())?
                } else {
                    task::generate(kind, lines, seed, dir.path())?
                };
                for &arm in &config.arms {
                    if let Some(limit) = config.budget_usd
                        && spent >= limit
                    {
                        stopped = Some(format!(
                            "the budget of ${limit:.2} is spent (${spent:.4})"
                        ));
                        break 'all;
                    }
                    let limits = Limits::default()
                        .max_turns(config.max_turns)
                        .timeout(config.timeout);
                    let agent = arm::agent(
                        arm,
                        llm.clone(),
                        &config.model,
                        dir.path(),
                        limits,
                    );
                    let started = Instant::now();
                    let mut meter = Meter::default();
                    let mut run = agent.start(&instance.prompt, &store);
                    {
                        let mut events = std::pin::pin!(run.events());
                        while let Some(event) = events.next().await {
                            meter.observe(&event);
                        }
                    }
                    let outcome = run.outcome().await;
                    let run = meter.finish(&outcome, started.elapsed());
                    spent += run.cost_usd;
                    let result = Trial {
                        kind,
                        lines,
                        bytes: instance.bytes,
                        arm,
                        trial,
                        expected: instance.expected.clone(),
                        score: task::score(
                            kind,
                            &instance.expected,
                            &run.reply,
                        ),
                        run,
                    };
                    done(&result);
                    trials.push(result);
                }
            }
        }
    }
    Ok(Report {
        model: config.model.clone(),
        summaries: metrics::summarize(&trials),
        trials,
        spent_usd: spent,
        stopped,
    })
}
