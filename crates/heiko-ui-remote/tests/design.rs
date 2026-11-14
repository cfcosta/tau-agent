//! heiko-ui-remote takes its look from `heiko-ui-kit`. Only `ui/components.rs`,
//! which holds the components that know heiko's own types, may write raw
//! design values (`heiko_ui_kit::design::check`).

use std::path::Path;

#[test]
fn only_the_design_files_hold_design_values() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = heiko_ui_kit::design::check(&src, &["ui/components.rs"]);
    assert!(
        found.is_empty(),
        "design values outside the design files:\n{}",
        found.join("\n")
    );
}
