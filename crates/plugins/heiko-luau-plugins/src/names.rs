//! heiko's built-in Luau plugins by name, for the Rust that must name one: a
//! kind's runs leaving one out, the demo, tests (ADR 0051). Plain strings, not
//! the list: `plugins/built-in.toml` is what heiko comes with.

pub const ASK: &str = "heiko-ask";
pub const COMPACTION: &str = "heiko-compaction";
pub const COMPACTION_CHAIN: &str = "heiko-compaction-chain";
pub const CONSTITUTION: &str = "heiko-constitution";
pub const FAST_COMPACTION: &str = "heiko-fast-compaction";
pub const GITHUB: &str = "heiko-github";
pub const GOAL: &str = "heiko-goal";
pub const MEMORY: &str = "heiko-memory";
pub const NOTIFY: &str = "heiko-notify";
pub const REASONING: &str = "heiko-reasoning";
pub const SCHEDULE: &str = "heiko-schedule";
pub const SKILLS: &str = "heiko-skills";
pub const SUGGESTIONS: &str = "heiko-suggestions";
pub const TITLES: &str = "heiko-titles";
pub const TODO: &str = "heiko-todo";
pub const TREE_COMPACTION: &str = "heiko-tree-compaction";
pub const WATCHER: &str = "heiko-watcher";

/// The first line of an output heiko-fast-compaction pruned, as its Luau
/// writes it.
pub const PRUNED_HEADER: &str = "[fast-compaction pruned this output: kept lines are verbatim, omitted lines are marked]";
