//! tau-todo (ADR 0048): issues a repository keeps for its runs to pick
//! up, like beads, and each run's own steps. It is a Luau plugin tau
//! comes with: its code is the Luau under `luau/`, which tau-luau-plugins
//! loads as always active; this crate carries the files, and its tests
//! check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-todo";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("lib/steps.luau", include_str!("../luau/lib/steps.luau")),
    ("lib/graph.luau", include_str!("../luau/lib/graph.luau")),
    ("lib/issues.luau", include_str!("../luau/lib/issues.luau")),
    ("lib/beads.luau", include_str!("../luau/lib/beads.luau")),
    ("lib/time.luau", include_str!("../luau/lib/time.luau")),
    ("tests/steps.luau", include_str!("../luau/tests/steps.luau")),
    (
        "tests/issues.luau",
        include_str!("../luau/tests/issues.luau"),
    ),
    ("tests/beads.luau", include_str!("../luau/tests/beads.luau")),
];
