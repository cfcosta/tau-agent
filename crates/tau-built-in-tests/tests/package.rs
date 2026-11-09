//! What keeps the package from breaking unseen (ADR 0051), as CI does not
//! build it: the flake names the plugins in the source and the install, and
//! checks the install. And what keeps a Luau edit from compiling anything:
//! no Rust embeds a file of `plugins/`.

use std::path::{Path, PathBuf};

/// Every file under `dir` named as `keep` likes, build output aside.
fn walk(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, keep, out);
        } else if keep(&path) {
            out.push(path);
        }
    }
}

/// Nothing Cargo tracks points at `plugins/`, so editing one rebuilds no
/// crate. Each plugin crate's own `plugins`-less paths are fine, as
/// `crates/plugins/...`; a path up and into the root's `plugins/` is not.
#[test]
fn no_rust_embeds_or_watches_a_plugin() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    walk(
        &crates,
        &|path| path.extension().is_some_and(|ext| ext == "rs"),
        &mut files,
    );
    assert!(files.len() > 100, "the sources are read");
    let mut naming = Vec::new();
    for file in files {
        // This file spells what it looks for.
        if file.ends_with("tests/package.rs") {
            continue;
        }
        let name = file.display().to_string();
        let is_build = file.ends_with("build.rs");
        let text = std::fs::read_to_string(&file).unwrap();
        for line in text.lines() {
            let embeds = line.contains("include_str!")
                || line.contains("include_bytes!");
            let points =
                line.contains("../plugins/") || line.contains("plugins/tau-");
            if (embeds && points) || (is_build && line.contains("plugins")) {
                naming.push(format!("{name}: {line}"));
            }
        }
    }
    assert_eq!(naming, Vec::<String>::new());
}

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
