//! A Luau plugin's jobs: work it does by itself, every so often, while
//! tau runs. The host starts each one through its jobs (ADR 0044), so
//! the person sees it going; what its `ctx.state` keeps, and when it
//! last ran, is in a file of its own, and outlives a restart. A tick, a
//! job that runs more often than [`TICK_UNDER`], is not shown, and its
//! file is written only when its state changes.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
pub use tau_luau_plugins::TICK_UNDER;
use tokio_util::sync::CancellationToken;

use crate::{
    registry::Registry,
    runtime::{Hook, JobRef},
    slots::{ModelReach, context},
};

/// How often the host looks for jobs that are due: as often as the
/// shortest may run.
pub const LOOK: Duration = Duration::from_secs(1);

/// What a job keeps between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    #[serde(default)]
    state: Value,
    /// When it last ran, in seconds since 1970.
    #[serde(default)]
    ran_at: u64,
}

fn file(dir: &Path, plugin: &str, job: &str) -> PathBuf {
    dir.join(format!("{plugin}.{job}.json"))
}

async fn kept(dir: &Path, plugin: &str, job: &str) -> Kept {
    tokio::fs::read_to_string(file(dir, plugin, job))
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Whether a job that runs `every` seconds and last ran at `ran_at` is
/// due at `now`.
pub fn due(ran_at: u64, every: u64, now: u64) -> bool {
    now.saturating_sub(ran_at) >= every
}

/// A job that is due: its plugin's name, its own, what it does, and how
/// often it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Due {
    pub plugin: String,
    pub job: String,
    pub label: String,
    pub every: u64,
}

/// The jobs of the active plugins that are due now, keeping their files
/// in `dir`; `ran` is when each last ran, read from its file the first
/// time, and each due one counts as run from now.
pub async fn due_now(
    registry: &Registry,
    dir: &Path,
    ran: &mut HashMap<(String, String), u64>,
) -> Vec<Due> {
    let mut due_now = Vec::new();
    let now = now();
    for active in registry.active().await {
        let plugin = &active.loaded.declaration.name;
        for job in &active.loaded.declaration.jobs {
            let key = (plugin.clone(), job.name.clone());
            let ran_at = match ran.get(&key) {
                Some(at) => *at,
                None => kept(dir, plugin, &job.name).await.ran_at,
            };
            if due(ran_at, job.every, now) {
                ran.insert(key, now);
                due_now.push(Due {
                    plugin: plugin.clone(),
                    job: job.name.clone(),
                    label: job.label.clone(),
                    every: job.every,
                });
            }
        }
    }
    due_now
}

/// What ticks left, by plugin and job, while tau runs: a tick reads its
/// file once, not each second.
#[derive(Debug, Default)]
pub struct Ticks(std::sync::Mutex<HashMap<(String, String), Value>>);

/// Runs `plugin`'s job `job` once, from what it kept in `dir` (a tick's
/// from `ticks`), and keeps what it left, reaching what `reach` gives a
/// job of a plugin with `uses.chats`. A job that fails still counts as
/// run: it waits its turn again, rather than fail again at once.
pub async fn run(
    registry: &Registry,
    plugin: &str,
    job: &str,
    dir: &Path,
    ticks: &Ticks,
    reach: JobRef,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let active = registry
        .plugin(plugin)
        .await
        .with_context(|| format!("{plugin} is not active"))?;
    let tick =
        active.loaded.declaration.jobs.iter().any(|declared| {
            declared.name == job && declared.every < TICK_UNDER
        });
    let key = (plugin.to_owned(), job.to_owned());
    let remembered = ticks.0.lock().expect("not poisoned").get(&key).cloned();
    let before = match remembered {
        Some(state) if tick => Kept { state, ran_at: 0 },
        _ => kept(dir, plugin, job).await,
    };
    let mut cx = context(registry, &active);
    // Its plugin's store of the app, the one its actions and pages use:
    // a job carries on what they began.
    cx.store = registry.store_of(&active.loaded.declaration, None).await;
    cx.state = if before.state.is_null() {
        json!({})
    } else {
        before.state.clone()
    };
    cx.job = Some(reach);
    let outcome = active
        .loaded
        .call(
            &Hook::Job(job.to_owned()),
            Value::Null,
            &cx,
            registry.with_files(
                &active.loaded.declaration,
                None,
                ModelReach::of(registry, &active, None),
            ),
            cancel,
        )
        .await;
    let after = Kept {
        state: outcome.state,
        ran_at: now(),
    };
    // A tick's time and state are kept in memory: written each second,
    // the file would only say it ran a second ago.
    if tick {
        ticks
            .0
            .lock()
            .expect("not poisoned")
            .insert(key, after.state.clone());
    }
    if !tick || after.state != before.state {
        tokio::fs::create_dir_all(dir).await?;
        tokio::fs::write(
            file(dir, plugin, job),
            serde_json::to_string(&after)?,
        )
        .await?;
    }
    match outcome.error {
        Some(error) => anyhow::bail!("{plugin}'s job {job}: {error}"),
        None => Ok(()),
    }
}
