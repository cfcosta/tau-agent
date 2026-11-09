//! What keeps the package from breaking unseen (ADR 0051), as CI does not
//! build it: the flake names the plugins in the source and the install, and
//! checks the install.

use std::path::Path;

#[test]
fn the_flake_ships_the_plugins_and_checks_them() {
    let flake = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../flake.nix");
    let flake = std::fs::read_to_string(flake).unwrap();
    for needed in [
        "./plugins\n",
        "cp -r plugins $out/share/tau/plugins",
        "doInstallCheck = true;",
        "$out/bin/tau --check-plugins",
        "$out/bin/tau --plugin-test",
    ] {
        assert!(flake.contains(needed), "flake.nix lacks {needed:?}");
    }
    // Under lazy trees `toString` copies the whole tree, target/ included.
    let copying = flake.lines().find(|line| {
        let code = line.split('#').next().unwrap_or("");
        code.contains("toString ./") || code.contains("toString .")
    });
    assert_eq!(copying, None, "flake.nix must not copy a path by toString");
}
