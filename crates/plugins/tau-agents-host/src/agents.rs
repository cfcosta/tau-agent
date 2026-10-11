//! The agents on this host (ADR 0037): their versions, and the overview
//! every interface draws of them.

use std::sync::Arc;

use tau_agents::{Agent, Overview, Settings, Status, Trigger};
use tau_ui_plugin::{HostCx, PluginHost};

use crate::{
    NAME,
    ROOT,
    clock::{Clock, SystemClock},
    registry::{Read, Registry},
};

/// tau-agents on the host. Clones share it.
#[derive(Clone)]
pub struct Agents {
    registry: Registry,
}

impl PluginHost for Agents {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        // The skill an agent's conversation reads to write itself, as
        // this build has it.
        let skills = cx.dir.join(tau_skills::BUILTIN_DIR);
        tokio::task::spawn_blocking(move || crate::skill::install(&skills))
            .await??;
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let refresher = cx.clone();
        let registry = Registry::at(
            cx.dir.join(ROOT),
            cx.plugin_dir(NAME).join("versions.json"),
            clock,
            move || refresher.refresh(),
            |_, _| {},
        );
        let watcher = registry.clone();
        cx.runtime.spawn(async move { watcher.watch().await });
        Ok(Self { registry })
    }
}

impl Agents {
    /// The agents as every interface shows them.
    pub async fn overview(&self, cx: &HostCx) -> Overview {
        let settings: Settings = cx.settings(NAME);
        Overview {
            agents: self
                .registry
                .agents()
                .await
                .into_iter()
                .map(|(name, read)| agent(name, &read, &settings))
                .collect(),
        }
    }
}

/// `read` as the interface shows it, with `settings`.
fn agent(name: String, read: &Read, settings: &Settings) -> Agent {
    let status = match (&read.live, &read.error) {
        (None, Some(error)) => Status::Broken {
            error: error.clone(),
        },
        _ => Status::Waiting,
    };
    let triggers = read
        .live
        .iter()
        .flat_map(|live| &live.declared.triggers)
        .map(|declared| Trigger {
            name: declared.name.clone(),
            kind: declared.kind.clone(),
            last: None,
            next: None,
        })
        .collect();
    Agent {
        name,
        purpose: read.purpose.clone(),
        status,
        triggers,
        versions: read.versions.clone(),
        spend_today: 0.0,
        cap_today: settings.daily_cap,
        queued: 0,
        last: None,
        model: settings.model(),
        effort: settings.effort().to_owned(),
    }
}
