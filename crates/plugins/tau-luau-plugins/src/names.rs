//! tau's built-in Luau plugins by name, for the Rust that must name one: a
//! kind's runs leaving one out, the demo, tests (ADR 0051). Plain strings, not
//! the list: `plugins/built-in.toml` is what tau comes with.

pub const ASK: &str = "tau-ask";
pub const CANVAS: &str = "tau-canvas";
pub const COMPACTION: &str = "tau-compaction";
pub const COMPACTION_CHAIN: &str = "tau-compaction-chain";
pub const CONSTITUTION: &str = "tau-constitution";
pub const FAST_COMPACTION: &str = "tau-fast-compaction";
pub const GITHUB: &str = "tau-github";
pub const GOAL: &str = "tau-goal";
pub const MEMORY: &str = "tau-memory";
pub const REASONING: &str = "tau-reasoning";
pub const SCHEDULE: &str = "tau-schedule";
pub const SKILLS: &str = "tau-skills";
pub const TITLES: &str = "tau-titles";
pub const TODO: &str = "tau-todo";
pub const TREE_COMPACTION: &str = "tau-tree-compaction";
pub const WATCHER: &str = "tau-watcher";

/// The first line of an output tau-fast-compaction pruned, as its Luau
/// writes it.
pub const PRUNED_HEADER: &str = "[fast-compaction pruned this output: kept lines are verbatim, omitted lines are marked]";
