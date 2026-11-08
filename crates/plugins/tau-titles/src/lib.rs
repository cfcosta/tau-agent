//! tau-titles (ADR 0045, 0050): it titles each run with one short
//! request to the plan's newest Luna at low effort. It is a Luau plugin
//! tau comes with: its code is the Luau under `luau/`, which
//! tau-luau-plugins loads as always active; this crate carries the
//! files, and its tests check them.

/// The plugin's name, and its folder's.
pub const NAME: &str = "tau-titles";

/// Its files, by their paths in its folder.
pub const FILES: &[(&str, &str)] = &[
    ("plugin.luau", include_str!("../luau/plugin.luau")),
    ("lib/titles.luau", include_str!("../luau/lib/titles.luau")),
    (
        "tests/titles.luau",
        include_str!("../luau/tests/titles.luau"),
    ),
];
