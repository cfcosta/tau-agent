//! The Luau plugins tau comes with (ADR 0051): read from the folder
//! `tau_luau_plugins::packages::find` gives, in the order its `built-in.toml`
//! lists them. The compactions rewrite in that order, the first rewrite
//! winning: fast pruning, then the tree, then the summary.
//!
//! They provide what a run cannot go without, so a listed plugin missing, a
//! package not listed and a plugin that does not load are each an error
//! tau does not start agents past.

use std::{collections::BTreeSet, path::Path};

use serde::Deserialize;
use tau_luau_plugins::packages;
use tokio::sync::OnceCell;

use crate::runtime::{Files, Loaded, MANIFEST, PLUGIN, load};

/// The file in the folder that lists them, in order.
pub const LIST: &str = "built-in.toml";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct List {
    plugins: Vec<String>,
}

/// The names `dir` lists, in order, checked against what it holds: it
/// holds each, and no package it does not list, the skills' examples
/// aside. It blocks.
pub fn listed(dir: &Path) -> Result<Vec<String>, String> {
    let path = dir.join(LIST);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let List { plugins } = toml::from_str(&text)
        .map_err(|error| format!("{}: {}", path.display(), error.message()))?;
    let names: BTreeSet<&str> = plugins.iter().map(String::as_str).collect();
    if names.len() != plugins.len() {
        return Err(format!("{}: it lists a plugin twice", path.display()));
    }
    for name in &plugins {
        if !dir.join(name).join(MANIFEST).is_file() {
            return Err(format!(
                "{} lists {name}, which {} does not hold",
                path.display(),
                dir.display()
            ));
        }
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    for entry in entries.flatten() {
        let folder = entry.file_name();
        let name = folder.to_string_lossy();
        if entry.path().is_dir()
            && !name.starts_with('.')
            && name != packages::EXAMPLES
            && !names.contains(name.as_ref())
        {
            return Err(format!(
                "{} holds {name}, which {} does not list",
                dir.display(),
                path.display()
            ));
        }
    }
    Ok(plugins)
}

/// Each plugin `dir` lists, as files, in order. It blocks.
pub fn read(dir: &Path) -> Result<Vec<Files>, String> {
    listed(dir)?
        .iter()
        .map(|name| {
            Files::read(&dir.join(name), PLUGIN)
                .map_err(|error| format!("the built-in {name}: {error}"))
        })
        .collect()
}

/// Loads what [`read`] read, in order.
pub async fn load_all(packages: Vec<Files>) -> Result<Vec<Loaded>, String> {
    let mut loaded = Vec::with_capacity(packages.len());
    for files in packages {
        let name = files.manifest.name.clone();
        loaded.push(load(&name, files).await.map_err(|error| {
            format!("the built-in {name} does not load: {error}")
        })?);
    }
    Ok(loaded)
}

/// What [`load_all`] gave of the folder [`packages::find`] names, once for
/// the process: a host takes these, and they are plain data.
pub async fn loaded() -> Result<Vec<Loaded>, String> {
    static LOADED: OnceCell<Result<Vec<Loaded>, String>> =
        OnceCell::const_new();
    LOADED
        .get_or_init(|| async {
            let packages =
                tokio::task::spawn_blocking(|| read(&packages::find()?))
                    .await
                    .map_err(|error| error.to_string())??;
            load_all(packages).await
        })
        .await
        .clone()
}

/// The built-in `name`'s files, as [`read`] has them: for what loads one
/// plugin apart, as a test. It blocks.
pub fn files(name: &str) -> Result<Files, String> {
    Files::read(&packages::find()?.join(name), PLUGIN)
}

/// Loads them before the host starts, so a failure stops agents with its
/// reason rather than leaving a host without compaction. It blocks: called
/// from `main`.
#[allow(
    clippy::disallowed_methods,
    reason = "called from main, before anything else runs (ADR 0028)"
)]
pub fn load_at_start() -> anyhow::Result<()> {
    runtime()
        .map_err(anyhow::Error::msg)?
        .block_on(loaded())
        .map(|_| ())
        .map_err(anyhow::Error::msg)
}

pub fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|error| error.to_string())
}
