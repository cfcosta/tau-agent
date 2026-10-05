//! tau-direnv's UI (ADR 0017): the question in the composer's place
//! ([`points::COMPOSER`]), the loading and failure cards above the
//! transcript ([`points::RUN_BANNER`]), and the toggle on the
//! repository's menu ([`points::REPO_MENU`]).

pub mod view;

use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_plugin::{
    Fold,
    HostCx,
    Manifest,
    PluginInfo,
    RepoCtx,
    RunCtx,
    RunCx,
    Seam,
    UiPlugin,
    points,
};

use crate::{Act, NAME, Record, RepoData, Settings};

/// tau-direnv with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct DirenvUi;

/// Where a run's workspace's environment stands, as tau-direnv's records
/// leave it: the last one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub now: Option<Record>,
}

impl Fold for State {
    type Record = Record;

    fn apply(&mut self, record: Record, _run: &mut dyn RunCx) {
        self.now = Some(record);
    }
}

/// The window's state: when a loading card last showed, so its clock
/// ticks while it does.
#[derive(Default)]
pub struct Ui {
    pub loading_shown: Option<Instant>,
    pub ticking: bool,
}

/// The host half, shared by every run.
#[cfg(feature = "host")]
pub type Host = crate::host::Host;
#[cfg(not(feature = "host"))]
pub type Host = ();

impl UiPlugin for DirenvUi {
    type State = State;
    type Data = ();
    type RepoData = RepoData;
    type Settings = Settings;
    type Host = Host;
    type Ui = Ui;

    fn name(&self) -> &'static str {
        NAME
    }

    /// The run hears of its workspace's environment. A sub-agent's
    /// commands go through it too, but no one sees the sub-agent's
    /// cards.
    async fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &Settings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        #[cfg(feature = "host")]
        {
            let dir = run.services.get::<tau_ui_plugin::WorkspaceDir>();
            match dir {
                Some(dir)
                    if host.installed()
                        && run.kind != tau_ui_plugin::RunKind::SubAgent =>
                {
                    Ok(vec![Box::new(host.plugin(&run.repo, dir.0.clone()))])
                }
                _ => Ok(Vec::new()),
            }
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (host, run);
            Ok(Vec::new())
        }
    }

    async fn launcher(
        &self,
        host: &Host,
        repo: &RepoCtx,
        _settings: &Settings,
    ) -> Option<std::sync::Arc<dyn tau_agent::launch::Launcher>> {
        #[cfg(feature = "host")]
        {
            host.launcher(repo)
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (host, repo);
            None
        }
    }

    async fn catalog(
        &self,
        _host: &Host,
        _cx: &HostCx,
        settings: &Settings,
    ) -> PluginInfo {
        let on = settings.repos.values().filter(|on| **on).count();
        PluginInfo {
            group: tau_ui_plugin::Group::Environment,
            note: (on > 0).then(|| {
                tau_ui_plugin::Note::new(
                    format!("on in {on}"),
                    tau_ui_kit::theme::Tone::Quiet,
                )
            }),
            description: "Runs agent commands in the repository's direnv \
                          environment, once you allow it"
                .into(),
            seams: vec![Seam::Start],
            page: None,
            ..Default::default()
        }
    }

    async fn repo_data(
        &self,
        host: &Host,
        repo: &RepoCtx,
        _cx: &HostCx,
    ) -> RepoData {
        #[cfg(feature = "host")]
        {
            host.repo_data(repo)
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (host, repo);
            RepoData::default()
        }
    }

    async fn act(
        &self,
        host: &Host,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        #[cfg(feature = "host")]
        {
            match act {
                Act::Decide { repo, load } => host.decide(&repo, load).await?,
                Act::Reload { run } => host.reload(&run)?,
            }
            Ok(None)
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = (host, act);
            anyhow::bail!("tau-direnv runs on the computer that runs the agent")
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
            .contribute(points::COMPOSER, view::question)
            .contribute(points::RUN_BANNER, view::card)
            .contribute(points::REPO_MENU, view::menu_entry)
            .settings(view::settings_pane)
    }
}
