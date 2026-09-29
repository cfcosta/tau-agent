//! What one trial measured, and the sums and means per workload.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::{runner::Bands, workload::Kind};

/// One workload, run once.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trial {
    pub workload: Kind,
    pub seed: u64,
    /// Lines the task needs.
    pub needles: usize,
    /// Of those, the ones in the result the model saw, verbatim as lines.
    pub retained: usize,
    /// Of those, the ones in what `bash` alone would have shown: the
    /// whole output, or its tail when it spilled.
    pub tail_retained: usize,
    /// The needles the model did not see.
    pub missed: Vec<String>,
    /// Lines of the whole output.
    pub lines: usize,
    /// Lines of the output the model saw, when the result was replaced.
    pub kept_lines: Option<usize>,
    /// Chunks the output went into, and how many stayed, when Jev was
    /// asked (the plugin's report), replaced or not.
    pub chunks: Option<usize>,
    pub kept_chunks: Option<usize>,
    /// Whether `bash` truncated the output and spilled it to a file.
    pub spilled: bool,
    /// Estimated tokens of the whole output, of what `bash` alone would
    /// have shown, and of what the model saw.
    pub tokens_full: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    /// Whether pruning replaced the result.
    pub replaced: bool,
    pub requests: usize,
    pub jev_input_tokens: u64,
    pub cost_usd: f64,
    /// Jev's answers about the chunks, by band.
    pub answers: Bands,
    pub latency_ms: u64,
    /// The file holding the output whole, when the result was replaced.
    pub archive: Option<String>,
    /// The plugin's error, when pruning failed.
    pub error: Option<String>,
}

impl Trial {
    /// The share of the estimated tokens pruning saved against what
    /// `bash` alone would have shown; 0 when it saved nothing.
    pub fn reduction(&self) -> f64 {
        if self.tokens_before == 0 {
            return 0.0;
        }
        (1.0 - self.tokens_after as f64 / self.tokens_before as f64).max(0.0)
    }
}

/// The trials of one workload, or of all (`workload: "total"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Summary {
    pub workload: String,
    pub trials: usize,
    pub needles: usize,
    pub retained: usize,
    pub tail_retained: usize,
    /// Mean of the trials' reductions.
    pub mean_reduction: f64,
    pub replaced: usize,
    pub errors: usize,
    pub requests: usize,
    /// Jev's answers about chunks, by band, summed.
    pub answers: Bands,
    pub cost_usd: f64,
    pub mean_latency_ms: f64,
}

impl Summary {
    /// Retained over needed; `None` when nothing was needed.
    pub fn recall(&self) -> Option<f64> {
        (self.needles > 0).then(|| self.retained as f64 / self.needles as f64)
    }

    pub fn tail_recall(&self) -> Option<f64> {
        (self.needles > 0)
            .then(|| self.tail_retained as f64 / self.needles as f64)
    }
}

fn summary_of(name: &str, trials: &[&Trial]) -> Summary {
    let count = trials.len();
    let mean = |total: f64| {
        if count == 0 {
            0.0
        } else {
            total / count as f64
        }
    };
    Summary {
        workload: name.to_owned(),
        trials: count,
        needles: trials.iter().map(|trial| trial.needles).sum(),
        retained: trials.iter().map(|trial| trial.retained).sum(),
        tail_retained: trials.iter().map(|trial| trial.tail_retained).sum(),
        mean_reduction: mean(
            trials.iter().map(|trial| trial.reduction()).sum(),
        ),
        replaced: trials.iter().filter(|trial| trial.replaced).count(),
        errors: trials.iter().filter(|trial| trial.error.is_some()).count(),
        requests: trials.iter().map(|trial| trial.requests).sum(),
        answers: Bands {
            noise: trials.iter().map(|trial| trial.answers.noise).sum(),
            uncertain: trials.iter().map(|trial| trial.answers.uncertain).sum(),
            needed: trials.iter().map(|trial| trial.answers.needed).sum(),
        },
        cost_usd: trials.iter().map(|trial| trial.cost_usd).sum(),
        mean_latency_ms: mean(
            trials.iter().map(|trial| trial.latency_ms as f64).sum(),
        ),
    }
}

/// One summary per workload that has trials, in [`Kind::ALL`]'s order,
/// then the total over every trial.
pub fn summarize(trials: &[Trial]) -> Vec<Summary> {
    let mut summaries: Vec<Summary> = Kind::ALL
        .iter()
        .filter_map(|kind| {
            let of: Vec<&Trial> = trials
                .iter()
                .filter(|trial| trial.workload == *kind)
                .collect();
            (!of.is_empty()).then(|| summary_of(kind.name(), &of))
        })
        .collect();
    summaries.push(summary_of("total", &trials.iter().collect::<Vec<_>>()));
    summaries
}

fn ratio(value: Option<f64>, retained: usize, needles: usize) -> String {
    match value {
        Some(value) => format!("{retained}/{needles} {:>4.0}%", value * 100.0),
        None => "—".to_owned(),
    }
}

/// The summaries as a plain-text table. `answers n/u/y` counts Jev's
/// answers about chunks: confident noise (at most 0.1, the only ones
/// that let a chunk go), uncertain, and needed (0.5 or more).
pub fn table(summaries: &[Summary]) -> String {
    let mut table = format!(
        "{:<20} {:>6} {:>12} {:>12} {:>9} {:>8} {:>6} {:>8} {:>17} {:>9} {:>8}\n",
        "workload",
        "trials",
        "recall",
        "tail recall",
        "reduction",
        "replaced",
        "errors",
        "requests",
        "answers n/u/y",
        "cost",
        "latency"
    );
    for summary in summaries {
        writeln!(
            table,
            "{:<20} {:>6} {:>12} {:>12} {:>8.1}% {:>8} {:>6} {:>8} {:>17} {:>9} {:>7.1}s",
            summary.workload,
            summary.trials,
            ratio(summary.recall(), summary.retained, summary.needles),
            ratio(summary.tail_recall(), summary.tail_retained, summary.needles),
            summary.mean_reduction * 100.0,
            summary.replaced,
            summary.errors,
            summary.requests,
            format!(
                "{}/{}/{}",
                summary.answers.noise,
                summary.answers.uncertain,
                summary.answers.needed
            ),
            format!("${:.4}", summary.cost_usd),
            summary.mean_latency_ms / 1000.0,
        )
        .expect("writing to a string");
    }
    table
}
