//! The retention evaluation of fast compaction's output pruning
//! (`docs/reference/fast-compaction.md`, "Evaluation").
//!
//! Each workload ([`workload`]) is a conversation that ends in one long
//! command output, generated from a seed, with the lines its task needs
//! (needles) among noise. The runner ([`runner`]) drives it through the
//! agent loop with the real plugin, the Luau one tau comes with, as the
//! app prunes, and measures ([`metrics`]) whether every needle reached
//! the model verbatim, how much smaller the result got, and what Jev
//! cost. [`measure`] is its own copy of how the plugin chunks an output
//! and estimates tokens.

pub mod measure;
pub mod metrics;
pub mod noise;
pub mod rng;
pub mod runner;
pub mod workload;
