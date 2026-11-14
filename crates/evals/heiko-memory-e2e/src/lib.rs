//! The end-to-end evaluation of heiko-memory (`docs/reference/plugins.md`,
//! `heiko-memory`, "Evaluation first"): coding tasks that need what an
//! earlier run found, before and after that fact changed, under six
//! arms (no memory, one `MEMORY.md`, search over the earlier run's raw
//! transcript, heiko-memory, heiko-memory with consolidation, and the
//! earlier run folded into a tree to zoom into), with the calls, tokens,
//! cost and time each takes. heiko-memory is the Luau plugin heiko comes
//! with, run as heiko runs it.
//!
//! [`scenario`] holds the tasks, [`arm`] builds each arm's agent,
//! [`runner`] runs the trials and [`metrics`] reads and sums them up.
//! The `heiko-memory-e2e` binary runs it all against a real model.

use std::path::PathBuf;

pub mod access;
pub mod arm;
pub mod bm25;
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
    /// heiko-memory, a Luau plugin, would not load or failed.
    #[error("heiko-memory: {0}")]
    Plugin(String),
    #[error(transparent)]
    Store(#[from] heiko_store::StoreError),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    /// Which trial failed, and why.
    #[error("{trial}: {source}")]
    Trial {
        trial: String,
        source: Box<E2eError>,
    },
}
