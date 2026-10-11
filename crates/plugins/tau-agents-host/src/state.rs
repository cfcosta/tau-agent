//! What tau keeps of each agent across restarts (ADR 0037): the events
//! waiting for its next turn, when its schedules and polls are due,
//! what each poll saw, where each stream was, and what its day's spend
//! is counted from.
//!
//! Each agent's is a JSON file of its own in tau-agents' directory,
//! written whole each time it changes. An agent's queue is short: a
//! turn takes every event that waits.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tau_agents::{Last, Notice, TriggerKind, event::Event};

/// One agent's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kept {
    /// Events waiting for a turn, oldest first.
    pub queue: Vec<Event>,
    /// Each schedule's next due time, by trigger.
    pub schedules: BTreeMap<String, Due>,
    /// When each trigger last fired, by trigger.
    pub fired: BTreeMap<String, i64>,
    /// The person paused it: events queue, and no turn starts.
    pub paused: bool,
    pub last: Option<Last>,
    /// The day, `YYYY-MM-DD`, its spend is counted for.
    pub day: Option<String>,
    /// What each of its runs had cost as that day began, by run.
    pub base: BTreeMap<String, f64>,
    /// When to try again a turn that did not start.
    pub retry_at: Option<i64>,
    /// Each poll's keys and when it polls next, by trigger.
    pub polls: BTreeMap<String, crate::poll::Polled>,
    /// Its notes and questions the person has not dealt with yet,
    /// oldest first.
    pub notices: Vec<Notice>,
    /// How many notices it ever left: the next one's id.
    pub noticed: u64,
    /// Each stream's last id, the ids it fired and its `parse`'s state,
    /// by trigger.
    pub streams: BTreeMap<String, crate::stream::Streamed>,
}

/// When a schedule is due next, and what it was when that was worked
/// out: a schedule that changes starts over.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Due {
    pub kind: TriggerKind,
    pub next: i64,
}

/// Where the agents' [`Kept`] files are.
#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn path(&self, agent: &str) -> PathBuf {
        self.dir.join(format!("{agent}.json"))
    }

    /// What is kept of `agent`; nothing yet is the default.
    pub async fn load(&self, agent: &str) -> Kept {
        let path = self.path(agent);
        tokio::task::spawn_blocking(move || read(&path))
            .await
            .unwrap_or_default()
    }

    /// Keeps `kept` as `agent`'s, whole or not at all.
    pub async fn save(&self, agent: &str, kept: &Kept) -> anyhow::Result<()> {
        let (path, text) = (self.path(agent), serde_json::to_string(kept)?);
        tokio::task::spawn_blocking(move || crate::write_atomic(&path, &text))
            .await??;
        Ok(())
    }
}

fn read(path: &Path) -> Kept {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}
