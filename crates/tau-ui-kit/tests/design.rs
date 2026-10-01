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
