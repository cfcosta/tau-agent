//! The kernel test (ADR 0045, step 12): no source of tau-ui or
//! tau-ui-remote, their tests apart, names a plugin. What still does is
//! listed here, each with why, and the list only shrinks: a name not on
//! it fails, and so does one on it that the source no longer has.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Where a plugin may still be named, and why. `*` is every plugin.
const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "tau-ui/src/hosted.rs",
        "*",
        "the host halves tau-ui gives the plugins: the app's composition, \
         as tau-plugins is the interface's",
    ),
];

fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_owned()
}

/// Every plugin, by the name its crate has in Rust: `tau_memory_host`.
fn plugins() -> BTreeSet<String> {
    std::fs::read_dir(crates().join("plugins"))
        .expect("crates/plugins")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("Cargo.toml").exists())
        .map(|entry| entry.file_name().to_string_lossy().replace('-', "_"))
        .collect()
}

fn sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a source directory") {
        let path = entry.expect("an entry").path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// `text` without its test modules (`#[cfg(test)]` before a `mod`, to
/// the brace that closes it) and its comment lines.
fn code(text: &str) -> String {
    let mut code = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("#[cfg(test)]") {
            // To the module, then to its closing brace.
            let mut opened = false;
            for line in lines.by_ref() {
                if !opened {
                    opened = line.trim_start().starts_with("mod ")
                        && line.trim_end().ends_with('{');
                    if line.trim_start().starts_with("mod ")
                        && line.trim_end().ends_with(';')
                    {
                        break;
                    }
                    continue;
                }
                if line == "}" {
                    break;
                }
            }
            continue;
        }
        if line.trim_start().starts_with("//") {
            continue;
        }
        code.push_str(line);
        code.push('\n');
    }
    code
}

/// Whether `code` names `plugin` as a path: `plugin::`.
fn names(code: &str, plugin: &str) -> bool {
    let needle = format!("{plugin}::");
    code.match_indices(&needle).any(|(at, _)| {
        code[..at]
            .chars()
            .last()
            .is_none_or(|before| !(before.is_alphanumeric() || before == '_'))
    })
}

#[test]
fn the_kernel_names_no_plugin_but_what_is_listed() {
    let plugins = plugins();
    let mut found: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for krate in ["tau-ui", "tau-ui-remote"] {
        let mut files = Vec::new();
        sources(&crates().join(krate).join("src"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).expect("a source");
            let code = code(&text);
            let relative = file
                .strip_prefix(crates())
                .expect("under crates/")
                .to_string_lossy()
                .into_owned();
            for plugin in &plugins {
                if names(&code, plugin) {
                    found
                        .entry(relative.clone())
                        .or_default()
                        .insert(plugin.clone());
                }
            }
        }
    }
    let allowed = |file: &str, plugin: &str| {
        ALLOWED.iter().any(|(at, name, _)| {
            *at == file && (*name == "*" || *name == plugin)
        })
    };
    let named: Vec<String> = found
        .iter()
        .flat_map(|(file, plugins)| {
            plugins
                .iter()
                .filter(|plugin| !allowed(file, plugin))
                .map(move |plugin| format!("{file} names {plugin}"))
        })
        .collect();
    assert!(
        named.is_empty(),
        "the kernel names plugins:\n{}",
        named.join("\n")
    );
    let stale: Vec<String> = ALLOWED
        .iter()
        .filter(|(file, plugin, _)| {
            *plugin != "*"
                && !found
                    .get(*file)
                    .is_some_and(|names| names.contains(*plugin))
        })
        .map(|(file, plugin, _)| format!("{file} no longer names {plugin}"))
        .collect();
    assert!(
        stale.is_empty(),
        "take these off the list:\n{}",
        stale.join("\n")
    );
}
