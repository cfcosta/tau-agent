//! The retention evaluation of fast compaction's output pruning
//! (`docs/reference/fast-compaction.md`, "Evaluation").
//!
//! Each workload ([`workload`]) is a conversation that ends in one long
//! command output, generated from a seed, with the lines its task needs
//! (needles) among noise. The runner ([`runner`]) drives it through the
//! agent loop with the real plugin, as the app prunes, and measures
//! ([`metrics`]) whether every needle reached the model verbatim, how
//! much smaller the result got, and what Jev cost.

pub mod metrics;
pub mod noise;
pub mod rng;
pub mod runner;
pub mod workload;
