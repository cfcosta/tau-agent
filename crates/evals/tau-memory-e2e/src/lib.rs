//! The end-to-end evaluation of tau-memory (`docs/reference/plugins.md`,
//! `tau-memory`, "Evaluation first"): coding tasks that need what an
//! earlier run found, before and after that fact changed, under five
//! arms (no memory, one `MEMORY.md`, search over the earlier run's raw
//! transcript, tau-memory, and tau-memory with consolidation), with the
//! calls, tokens, cost and time each takes.
//!
//! [`scenario`] holds the tasks, [`arm`] builds each arm's agent,
//! [`runner`] runs the trials and [`metrics`] reads and sums them up.
//! The `tau-memory-e2e` binary runs it all against a real model.

use std::path::PathBuf;

pub mod access;
pub mod arm;
pub mod metrics;
pub mod runner;
pub mod scenario;

/// Why a trial could not run to its end.
#[derive(Debug, thiserror::Error)]
pub enum E2eError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot run bash: {0}")]
    Bash(#[source] std::io::Error),
    #[error("the script failed in {}:\n{script}\n{output}", dir.display())]
    Script {
        dir: PathBuf,
        script: String,
        output: String,
    },
    #[error("the check stopped: {0}")]
    Stopped(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Memory(#[from] tau_memory_host::memory::OpenError),
    #[error(transparent)]
    Index(#[from] tau_memory_host::index::IndexError),
    #[error(transparent)]
    Notes(#[from] tau_memory_host::store::StoreError),
    #[error(transparent)]
    Store(#[from] tau_store::StoreError),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    /// Which trial failed, and why.
    #[error("{trial}: {source}")]
    Trial {
        trial: String,
        source: Box<E2eError>,
    },
}
