//! The kernel names no plugin (ADR 0045): what a plugin adds, it adds
//! through the points and the slots, and tau-plugins lists them. Neither
//! tau-ui-remote's sources outside their tests nor the phone's name a
//! plugin's crate, its name or its tools, and neither links a plugin
//! crate, so a phone runs none of their code (ADR 0046).

use std::path::{Path, PathBuf};

/// Every Rust file under `dir`, with its text up to its tests: a file's
/// `#[cfg(test)]` module is its last item.
fn sources(dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let code = match text.find("#[cfg(test)]") {
                Some(at) => text[..at].to_owned(),
                None => text,
            };
            out.push((path.display().to_string(), code));
        }
    }
}

/// The workspace's root.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_owned()
}

/// Every plugin crate's name, as its manifest and its paths spell it:
/// `tau-goal` and `tau_goal`.
fn plugins() -> Vec<(String, String)> {
    // The Rust plugins, and the Luau ones tau comes with (ADR 0051).
    let mut names: Vec<(String, String)> = ["crates/plugins", "plugins"]
        .iter()
        .flat_map(|dir| std::fs::read_dir(root().join(dir)).unwrap())
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .map(|name| (name.clone(), name.replace('-', "_")))
        .collect();
    names.sort();
    names
}

/// The names of plugins' tools that read like no other word.
const TOOLS: [&str; 4] = ["exec_command", "write_stdin", "apply_patch", "vcs_"];

#[test]
fn no_kernel_source_names_a_plugin() {
    let mut files = Vec::new();
    sources(&root().join("crates/tau-ui-remote/src"), &mut files);
    sources(&root().join("crates/tau-phone/src"), &mut files);
    assert!(files.len() > 10, "the sources are read");
    let plugins = plugins();
    assert!(plugins.len() > 20, "the plugins are listed");
    let mut naming = Vec::new();
    for (path, code) in &files {
        for (name, path_name) in &plugins {
            for word in [path_name.clone(), format!("\"{name}\"")] {
                if code.contains(&word) {
                    naming.push(format!("{path}: {word}"));
                }
            }
        }
        for tool in TOOLS {
            if code.contains(&format!("\"{tool}")) {
                naming.push(format!("{path}: {tool}"));
            }
        }
    }
    assert_eq!(naming, Vec::<String>::new());
}

/// What `manifest` depends on outside its tests.
fn dependencies(manifest: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(manifest).unwrap();
    let mut names = Vec::new();
    let mut in_dependencies = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_dependencies = line.ends_with("dependencies]")
                && !line.contains("dev-dependencies");
            continue;
        }
        if in_dependencies
            && !line.starts_with('#')
            && let Some((name, _)) = line.split_once(['=', '.'])
        {
            names.push(name.trim().to_owned());
        }
    }
    names
}

#[test]
fn neither_the_kernel_nor_the_phone_links_a_plugin() {
    let plugins = plugins();
    for krate in ["tau-ui-remote", "tau-phone"] {
        let manifest = root().join("crates").join(krate).join("Cargo.toml");
        let linked = dependencies(&manifest);
        assert!(
            linked.contains(&"tau-remote".to_owned()),
            "{krate}'s are read"
        );
        let named: Vec<&String> = linked
            .iter()
            .filter(|name| plugins.iter().any(|(plugin, _)| plugin == *name))
            .collect();
        assert_eq!(named, Vec::<&String>::new(), "{krate} links a plugin");
    }
}
