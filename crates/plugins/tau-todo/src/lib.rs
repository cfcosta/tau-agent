//! tau-todo (ADR 0048): issues a repository keeps for its runs to pick
//! up, like beads, and each run's own steps. It is a Luau plugin tau
//! comes with: its code is the Luau under `plugins/tau-todo/`, which tau-luau-plugins
//! loads as always active; this crate carries the files, and its tests
//! check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-todo";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    (
        "tau.toml",
        include_str!("../../../../plugins/tau-todo/tau.toml"),
    ),
    (
        "src/plugin.luau",
        include_str!("../../../../plugins/tau-todo/src/plugin.luau"),
    ),
    (
        "src/steps.luau",
        include_str!("../../../../plugins/tau-todo/src/steps.luau"),
    ),
    (
        "src/graph.luau",
        include_str!("../../../../plugins/tau-todo/src/graph.luau"),
    ),
    (
        "src/issues.luau",
        include_str!("../../../../plugins/tau-todo/src/issues.luau"),
    ),
    (
        "src/beads.luau",
        include_str!("../../../../plugins/tau-todo/src/beads.luau"),
    ),
    (
        "src/time.luau",
        include_str!("../../../../plugins/tau-todo/src/time.luau"),
    ),
    (
        "tests/steps.luau",
        include_str!("../../../../plugins/tau-todo/tests/steps.luau"),
    ),
    (
        "tests/issues.luau",
        include_str!("../../../../plugins/tau-todo/tests/issues.luau"),
    ),
    (
        "tests/beads.luau",
        include_str!("../../../../plugins/tau-todo/tests/beads.luau"),
    ),
    (
        "tests/graph.luau",
        include_str!("../../../../plugins/tau-todo/tests/graph.luau"),
    ),
];
