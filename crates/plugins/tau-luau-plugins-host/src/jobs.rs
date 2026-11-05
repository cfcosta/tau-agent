//! A Luau plugin's jobs: work it does by itself, every so often, while
//! tau runs. The host starts each one through its jobs (ADR 0044), so
//! the person sees it going; what its `ctx.state` keeps, and when it
//! last ran, is in a file of its own, and outlives a restart.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use crate::{
    registry::Registry,
    runtime::Hook,
    slots::{ModelReach, context},
};

/// How often the host looks for jobs that are due.
pub const LOOK: Duration = Duration::from_secs(30);

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

/// The jobs of the active plugins that are due now, keeping their files
/// in `dir`: each plugin's name and the job's.
pub async fn due_now(
    registry: &Registry,
    dir: &Path,
) -> Vec<(String, String, String)> {
    let mut due_now = Vec::new();
    let now = now();
    for active in registry.active().await {
        let plugin = &active.loaded.declaration.name;
        for job in &active.loaded.declaration.jobs {
            if due(kept(dir, plugin, &job.name).await.ran_at, job.every, now) {
                due_now.push((
                    plugin.clone(),
                    job.name.clone(),
                    job.label.clone(),
                ));
            }
        }
    }
    due_now
}

/// Runs `plugin`'s job `job` once, from what it kept in `dir`, and keeps
/// what it left. A job that fails still counts as run: it waits its turn
/// again, rather than fail again at once.
pub async fn run(
    registry: &Registry,
    plugin: &str,
    job: &str,
    dir: &Path,
    cancel: CancellationToken,
) -> anyhow::Result<()> {
    let active = registry
        .plugin(plugin)
        .await
        .with_context(|| format!("{plugin} is not active"))?;
    let before = kept(dir, plugin, job).await;
    let mut cx = context(registry, &active);
    cx.state = if before.state.is_null() {
        json!({})
    } else {
        before.state
    };
    let outcome = active
        .loaded
        .call(
            &Hook::Job(job.to_owned()),
            Value::Null,
            &cx,
            ModelReach::of(registry, &active, None),
            cancel,
        )
        .await;
    let after = Kept {
        state: outcome.state,
        ran_at: now(),
    };
    tokio::fs::create_dir_all(dir).await?;
    tokio::fs::write(file(dir, plugin, job), serde_json::to_string(&after)?)
        .await?;
    match outcome.error {
        Some(error) => anyhow::bail!("{plugin}'s job {job}: {error}"),
        None => Ok(()),
    }
}
