//! The kit holds the design values: only its theme and components may.

use std::path::Path;

#[test]
fn only_the_design_files_hold_design_values() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(
        &src,
        &["theme.rs", "components.rs", "design.rs"],
    );
    assert!(
        found.is_empty(),
        "design values outside the design files:\n{}",
        found.join("\n")
    );
}

/// Every plugin draws with the kit: its sources hold no design values
/// of their own (ADR 0017).
#[test]
fn only_the_kit_holds_the_plugins_design_values() {
    let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins");
    let mut checked = 0;
    for plugin in std::fs::read_dir(&plugins).unwrap() {
        let src = plugin.unwrap().path().join("src");
        if !src.is_dir() {
            continue;
        }
        checked += 1;
        let found = tau_ui_kit::design::check(&src, &[]);
        assert!(
            found.is_empty(),
            "design values in {}:\n{}",
            src.display(),
            found.join("\n")
        );
    }
    assert!(checked > 0, "no plugins under {}", plugins.display());
}
