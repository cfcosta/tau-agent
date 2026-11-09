//! What the tests in the loop share: tau-fast-compaction as tau runs it,
//! its Luau run by tau-luau-plugins, on a `FakeJev` the tests choose.

#![allow(dead_code, reason = "each test file uses some of them")]

use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use serde_json::{Value, json};
use tau_agent::event::RunEvent;
use tau_jev::fake::FakeJev;
use tau_luau_plugins::Record;
use tau_luau_plugins_host::{
    agent::{Active, LuauPlugins},
    runtime::{Loaded, load},
};
use tau_store::{Entry, Store};
use tau_testing::block_on_io;

/// `future`'s output, on a thread of its own: the tests build their
/// agents inside the runtime that drives them.
fn aside<T: Send>(future: impl std::future::Future<Output = T> + Send) -> T {
    std::thread::scope(|scope| {
        scope.spawn(|| block_on_io(future)).join().unwrap()
    })
}

/// tau-fast-compaction, loaded once as tau loads it.
fn loaded() -> Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            let files = tau_luau_plugins_host::built_in::files(
                tau_luau_plugins::names::FAST_COMPACTION,
            )
            .unwrap();
            aside(load(tau_luau_plugins::names::FAST_COMPACTION, files))
                .unwrap()
        })
        .clone()
}

/// The plugin as tau runs it, `settings` over its defaults, asking
/// `jev`, archiving to `archive`.
pub fn fast(settings: Value, jev: &FakeJev, archive: &Path) -> LuauPlugins {
    let loaded = loaded();
    let mut merged = loaded.declaration.default_settings();
    for (key, value) in settings.as_object().unwrap() {
        merged[key] = value.clone();
    }
    LuauPlugins::new(
        vec![Active {
            loaded,
            settings: merged,
        }],
        json!({ "kind": "main", "repo": "r", "model": "m" }),
        Some(Arc::new(jev.clone())),
    )
    .with_archive(archive.to_owned())
}

/// A fresh directory for one test's archives.
pub fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CASE: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "tau-fast-compaction-{name}-{}-{}",
        std::process::id(),
        CASE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Which Luau plugin made each rewrite, as its stored details say, and
/// the details.
pub async fn rewrites(store: &Store, run: &str) -> Vec<(String, Value)> {
    store
        .transcript(run)
        .await
        .unwrap()
        .iter()
        .filter_map(|entry| match entry {
            Entry::Context { body, .. } => {
                let body: Value = serde_json::from_str(body).unwrap();
                Some((
                    body["plugin"].as_str().unwrap().to_owned(),
                    body["details"].clone(),
                ))
            }
            _ => None,
        })
        .collect()
}

/// The plugin's last state in the run `run`.
pub async fn state(store: &Store, run: &str) -> Value {
    records(store, run)
        .await
        .into_iter()
        .rev()
        .find_map(|record| match record {
            Record::State { plugin, state }
                if plugin == tau_luau_plugins::names::FAST_COMPACTION =>
            {
                Some(state)
            }
            _ => None,
        })
        .unwrap_or_default()
}

/// What tau-luau-plugins stored with the run `run`.
pub async fn records(store: &Store, run: &str) -> Vec<Record> {
    store
        .records(run, tau_luau_plugins::NAME)
        .await
        .unwrap()
        .iter()
        .filter_map(|body| serde_json::from_str(body).ok())
        .collect()
}

/// The plugin's log lines in the run `run`.
pub async fn logs(store: &Store, run: &str) -> Vec<String> {
    records(store, run)
        .await
        .into_iter()
        .filter_map(|record| match record {
            Record::Log { plugin, lines }
                if plugin == tau_luau_plugins::names::FAST_COMPACTION =>
            {
                Some(lines)
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// What the run's plugins charged Jev for, from its events.
pub fn charged(events: &[RunEvent]) -> f64 {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::PluginCharged { plugin, usage, .. }
                if &**plugin == tau_luau_plugins::NAME =>
            {
                Some(usage.cost.total)
            }
            _ => None,
        })
        .sum()
}
