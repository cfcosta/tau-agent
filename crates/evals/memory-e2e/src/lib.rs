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

pub mod access;
pub mod arm;
pub mod metrics;
pub mod runner;
pub mod scenario;
