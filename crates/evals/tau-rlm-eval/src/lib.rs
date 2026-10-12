//! A recursive-language-model evaluation (Zhang, Kraska and Khattab,
//! "Recursive Language Models", arXiv 2512.24601): long inputs held in a
//! file, tasks whose answers depend on every line, and agents that must
//! find a way through them. [`task`] generates the inputs and scores the
//! answers, [`arm`] builds each arm's agent, [`runner`] runs the trials
//! against a real model and [`metrics`] sums them up.
//!
//! The arms differ in what the model can do about an input too large to
//! read: only the coding tools; codemode with `tools.infer` and its
//! default limits; and codemode with limits raised and the strategy of
//! the paper in the instructions (chunk, fan out, aggregate in code).

use std::path::PathBuf;

pub mod access;
pub mod arm;
pub mod metrics;
pub mod rng;
pub mod runner;
pub mod task;

/// Why a trial could not run to its end.
#[derive(Debug, thiserror::Error)]
pub enum EvalError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Store(#[from] tau_store::StoreError),
    /// Which trial failed, and why.
    #[error("{trial}: {source}")]
    Trial {
        trial: String,
        source: Box<EvalError>,
    },
}
