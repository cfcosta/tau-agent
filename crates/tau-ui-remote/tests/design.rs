//! tau-ui-remote takes its look from `tau-ui-kit`. Only `ui/components.rs`,
//! which holds the components that know tau's own types, may write raw
//! design values (`tau_ui_kit::design::check`).

use std::path::Path;

#[test]
fn only_the_design_files_hold_design_values() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(&src, &["ui/components.rs"]);
    assert!(
        found.is_empty(),
        "design values outside the design files:\n{}",
        found.join("\n")
    );
}
