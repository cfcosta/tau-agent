//! tau-tree-compaction (docs/reference/tree-compaction.md, ADR 0050):
//! compaction into a tree of one-line summaries the agent can zoom into,
//! after OptChat. Near the window, the messages before the kept tail are
//! folded into a history kept word for word, a binary tree of summaries
//! grows over it, and its view takes their place; the `zoom` tool opens
//! any line into the two it was made from, down to a message whole. Off
//! by default. It is a Luau plugin tau comes with: its code is the Luau
//! under `luau/`, which tau-luau-plugins loads as always active; this
//! crate carries the files and the tests that check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-tree-compaction";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("lib/tree.luau", include_str!("../luau/lib/tree.luau")),
    ("lib/build.luau", include_str!("../luau/lib/build.luau")),
    ("tests/build.luau", include_str!("../luau/tests/build.luau")),
    (
        "tests/plugin.luau",
        include_str!("../luau/tests/plugin.luau"),
    ),
    ("tests/tree.luau", include_str!("../luau/tests/tree.luau")),
];

/// What the plugin declares, as the demo lists it among tau's Luau
/// plugins.
pub const DEMO_DECLARATION: &str = include_str!("../fixtures/declaration.json");
