//! tau-compaction-chain (ADR 0045, 0050): it fills `tau.compaction` with
//! fast pruning, the tree and the summary, which rewrite in that turn. It
//! is a Luau plugin tau comes with: its code is the Luau under `luau/`,
//! which tau-luau-plugins loads as always active; this crate carries the
//! files, and its tests check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-compaction-chain";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("tests/chain.luau", include_str!("../luau/tests/chain.luau")),
];
