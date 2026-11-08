//! tau-fast-compaction (docs/reference/fast-compaction.md, ADR 0050):
//! keeps a run's context lean with Jev, in two stages. A large
//! `exec_command` or `write_stdin` result is trimmed to the lines the
//! task still needs before the model first sees it, the whole output
//! archived; and between turns, stale tool calls and results are dropped
//! or cut, as a context rewrite. It is a Luau plugin tau comes with: its
//! code is the Luau under `luau/`, which tau-luau-plugins loads as always
//! active; this crate carries the files and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-fast-compaction";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("lib/decide.luau", include_str!("../luau/lib/decide.luau")),
    (
        "lib/estimate.luau",
        include_str!("../luau/lib/estimate.luau"),
    ),
    ("lib/history.luau", include_str!("../luau/lib/history.luau")),
    ("lib/ledger.luau", include_str!("../luau/lib/ledger.luau")),
    ("lib/output.luau", include_str!("../luau/lib/output.luau")),
    ("lib/plan.luau", include_str!("../luau/lib/plan.luau")),
    ("lib/state.luau", include_str!("../luau/lib/state.luau")),
    (
        "tests/estimate.luau",
        include_str!("../luau/tests/estimate.luau"),
    ),
    (
        "tests/output.luau",
        include_str!("../luau/tests/output.luau"),
    ),
    (
        "tests/plugin.luau",
        include_str!("../luau/tests/plugin.luau"),
    ),
    ("tests/rules.luau", include_str!("../luau/tests/rules.luau")),
];

/// The first line of a pruned output, as the plugin writes it.
pub const HEADER: &str = "[fast-compaction pruned this output: kept lines are verbatim, omitted lines are marked]";

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str = include_str!("../fixtures/declaration.json");

/// The demo run's three steps, as the plugin keeps and draws them, as
/// JSON (`[{ state, view }]`): started with a key, the test run's output
/// cut as it arrived, the history pruned between turns.
pub const DEMO_RUN: &str = include_str!("../fixtures/run.json");
