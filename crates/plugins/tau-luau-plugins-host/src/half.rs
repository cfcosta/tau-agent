//! tau-luau-plugins' host half (ADR 0030).

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_luau_plugins::{
    Act,
    LuauPluginsUi,
    LuauSettings,
    Overview,
    Standing,
    ui::PAGE,
};
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{HostCx, HostHalf, Link, PluginInfo, RepoCtx, RunCtx};

/// tau-luau-plugins on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct LuauPluginsHost;

/// The versions one run has active, with the person's settings: trunk's,
/// or for a run in the plugins repository, its workspace's where they
/// may be.
struct RunVersions {
    registry: crate::registry::Registry,
    settings: LuauSettings,
    workspace: Option<std::path::PathBuf>,
    /// The plugins the run's repository leaves out, by name.
    left_out: &'static [&'static str],
}

#[async_trait::async_trait]
impl crate::agent::Versions for RunVersions {
    async fn now(&self) -> Vec<crate::agent::Active> {
        let active = match &self.workspace {
            Some(dir) => self.registry.in_workspace(dir).await,
            None => self.registry.active().await,
        };
        active
            .into_iter()
            .filter(|active| {
                !self
                    .left_out
                    .contains(&active.loaded.declaration.name.as_str())
            })
            .map(|mut active| {
                let name = &active.loaded.declaration.name;
                active.settings = crate::settings::effective(
                    &active.loaded.declaration,
                    self.settings.plugins.get(name),
                )
                .0;
                active
            })
            .collect()
    }
}

impl HostHalf for LuauPluginsHost {
    type Plugin = LuauPluginsUi;
    type Host = crate::registry::Registry;

    async fn agent_plugins(
        &self,
        host: &Self::Host,
        run: &RunCtx,
        settings: &LuauSettings,
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();
        // A run in the plugins repository tests plugins as its workspace
        // has them, and runs them from there as soon as they pass.
        let workspace = (run.repo.name == crate::REPO)
            .then(|| run.services.get::<tau_ui_plugin::WorkspaceDir>())
            .flatten()
            .map(|dir| dir.0.clone());
        if let Some(dir) = &workspace {
            plugins.push(Box::new(crate::testing::PluginTesting::new(
                dir.clone(),
            )));
        }
        let kind = match run.kind {
            tau_ui_plugin::RunKind::Main => "main",
            tau_ui_plugin::RunKind::Chat => "chat",
            tau_ui_plugin::RunKind::SubAgent => "sub_agent",
        };
        let info = serde_json::json!({
            "kind": kind,
            "repo": run.repo.name,
            "model": run.model,
        });
        let jev = run
            .services
            .get::<std::sync::Arc<dyn tau_jev::Jev>>()
            .cloned();
        // Always there, even with no plugin active yet: one that becomes
        // active while the run goes on works in it from its next request.
        let versions = RunVersions {
            registry: host.clone(),
            settings: settings.clone(),
            workspace,
            // An agent's runs ask through tau-agents' own `ask`, not
            // tau-ask's (ADR 0037).
            left_out: run
                .services
                .get::<tau_ui_plugin::LeftOut>()
                .map_or(&[][..], |left_out| left_out.0),
        };
        let luau = crate::agent::LuauPlugins::live(
            std::sync::Arc::new(versions),
            info,
            jev,
        )
        .await;
        // Their HTTP requests name secrets kept for each in tau's
        // configuration directory.
        let luau = luau.with_stores(host.stores());
        // What they cut from the context, archived in tau's directory
        // for the repository, readable by the person alone.
        let luau = luau.with_archive(run.repo.dir.join("archive"));
        // What their tools ask the person, carried out once allowed.
        let luau = luau.with_dispatch(crate::agent::Dispatch {
            allows: host.allows(),
            forms: host.forms(),
            registry: host.clone(),
            chats: host.chats(),
        });
        let luau = match run.services.get::<tau_ui_plugin::ConfigDir>() {
            Some(config) => luau.with_config(config.0.clone()),
            None => luau,
        };
        plugins.push(Box::new(luau));
        Ok(plugins)
    }

    /// Starts the Luau plugins' jobs as they come due, for as long as
    /// tau runs: each through the host's jobs, which show it going.
    async fn resume(&self, host: &Self::Host, cx: &HostCx) {
        // What the plugins held for runs the last tau left cut off goes.
        let repos: Vec<String> =
            cx.repos.iter().map(|repo| repo.name.clone()).collect();
        host.recover(&repos, cx.interrupted()).await;
        let Some(jobs) = cx.jobs().cloned() else {
            return;
        };
        let dir = cx.plugin_dir(crate::NAME).join("jobs");
        let registry = host.clone();
        cx.runtime.spawn(async move {
            loop {
                for (plugin, job, label) in
                    crate::jobs::due_now(&registry, &dir).await
                {
                    let (registry, dir) = (registry.clone(), dir.clone());
                    let (named, what) = (plugin.clone(), job.clone());
                    // One already going waits for its next turn.
                    let _ = jobs.start(
                        crate::NAME,
                        &format!("{plugin}/{job}"),
                        format!("{plugin}: {label}"),
                        move |job_cx| async move {
                            crate::jobs::run(
                                &registry,
                                &named,
                                &what,
                                &dir,
                                job_cx.cancel_token().clone(),
                            )
                            .await
                        },
                    );
                }
                tokio::time::sleep(crate::jobs::LOOK).await;
            }
        });
    }

    /// Fills the slots a Luau plugin may fill with stand-ins that ask
    /// the active one that does.
    fn fill(&self, host: &Self::Host, fills: &mut tau_ui_plugin::slots::Fills) {
        fills.fill(
            &tau_ui_plugin::title::TITLE,
            std::sync::Arc::new(crate::slots::LuauTitle(host.clone()))
                as std::sync::Arc<dyn tau_ui_plugin::title::Title>,
        );
        fills.fill(
            &tau_ui_plugin::forge::FORGE,
            std::sync::Arc::new(crate::slots::LuauForge(host.clone()))
                as std::sync::Arc<dyn tau_ui_plugin::forge::Forge>,
        );
        fills.fill(
            &tau_ui_plugin::notify::NOTIFY,
            std::sync::Arc::new(crate::slots::LuauNotify(host.clone()))
                as std::sync::Arc<dyn tau_ui_plugin::notify::Notify>,
        );
        fills.fill(
            &tau_ui_plugin::compaction::COMPACTION,
            std::sync::Arc::new(crate::slots::LuauCompaction(host.clone()))
                as std::sync::Arc<dyn tau_ui_plugin::compaction::Compaction>,
        );
    }

    /// A chat in the plugins repository lands by itself once its
    /// plugins pass and want nothing new (ADR 0034).
    async fn lands_itself(
        &self,
        host: &Self::Host,
        repo: &RepoCtx,
        workspace: &std::path::Path,
    ) -> bool {
        repo.name == crate::REPO && host.ready(workspace).await
    }

    async fn catalog(
        &self,
        host: &Self::Host,
        _cx: &HostCx,
        _settings: &LuauSettings,
    ) -> PluginInfo {
        let description = {
            let overview = host.overview().await;
            let waiting = overview
                .plugins
                .iter()
                .filter(|entry| !matches!(entry.standing, Standing::Active))
                .count();
            match (overview.plugins.len(), waiting) {
                (0, _) => "Plugins written in Luau: none yet".to_owned(),
                (all, 0) => format!("Plugins written in Luau: {all} active"),
                (all, waiting) => {
                    format!(
                        "Plugins written in Luau: {all}, {waiting} need a look"
                    )
                }
            }
        };
        // Each plugin of the repository is a row of its own.
        let entries: Vec<tau_ui_plugin::CatalogEntry> = host
            .overview()
            .await
            .plugins
            .iter()
            .map(entry_row)
            .collect();
        PluginInfo {
            group: tau_ui_plugin::Group::Yours,
            description,
            page: Some(Link::page(PAGE)),
            entries,
            ..Default::default()
        }
    }

    async fn data(&self, host: &Self::Host, _cx: &HostCx) -> Overview {
        host.overview().await
    }

    /// What each Luau plugin keeps of `repo` (`repo_data`).
    async fn repo_data(
        &self,
        host: &Self::Host,
        repo: &RepoCtx,
        _cx: &HostCx,
    ) -> tau_luau_plugins::RepoValues {
        host.repo_data(&repo.name).await
    }

    async fn act(
        &self,
        host: &Self::Host,
        action: Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let action: Act = serde_json::from_value(action)?;
        let chats = cx.conversations();
        let chats = chats.as_deref();
        // The host's Jev, for an action of a plugin that uses it.
        let host_jev = || {
            cx.services
                .get::<std::sync::Arc<dyn tau_jev::Jev>>()
                .cloned()
        };
        match action {
            Act::Allow { plugin } => host.allow(&plugin).await?,
            Act::Page {
                plugin,
                page,
                params,
                jev,
            } => {
                return Ok(Some(host.page(&plugin, &page, &params, jev).await));
            }
            Act::RepoAction {
                plugin,
                repo,
                action,
                args,
            } => {
                host.repo_action(
                    &plugin,
                    &repo,
                    &action,
                    args,
                    chats,
                    host_jev(),
                )
                .await?;
            }
            Act::Answer {
                run,
                call,
                allowed,
                reason,
            } => {
                // From whichever device answers first; a later answer, or
                // one for a run that ended, finds nothing to take.
                let _ = host.allows().answer(
                    &run,
                    &call,
                    crate::chats::Answer { allowed, reason },
                );
            }
            Act::SetSecret {
                plugin,
                name,
                value,
            } => host.set_secret(&plugin, &name, &value).await?,
            Act::SettingsView {
                plugin,
                settings,
                fields,
            } => {
                let page = host.settings_page(&plugin, settings.clone()).await;
                let tree = tau_luau_plugins::pane::settings_view(
                    page.page, &settings, &fields,
                );
                return Ok(Some(tree));
            }
            Act::Action {
                plugin,
                action,
                args,
                run,
                repo,
                state,
                settings,
                live,
                jev,
            } => {
                let done = host
                    .action(crate::registry::ActionCall {
                        plugin: &plugin,
                        action: &action,
                        args: &args,
                        run: &run,
                        repo: repo.as_deref(),
                        state: &state,
                        settings: settings.as_ref(),
                        chats,
                        jev: host_jev(),
                        live,
                        keyed: jev,
                    })
                    .await;
                return Ok(Some(serde_json::to_value(done)?));
            }
        }
        Ok(None)
    }
}

/// A plugin of the repository as a row of the Plugins screen: where it
/// steps in, and a word when it needs the person.
fn entry_row(entry: &crate::Entry) -> tau_ui_plugin::CatalogEntry {
    use tau_ui_plugin::{CatalogEntry, Note};

    let seams = entry
        .declaration
        .as_ref()
        .map(tau_luau_plugins::Declaration::seams)
        .unwrap_or_default();
    let note = match &entry.standing {
        Standing::Active => None,
        Standing::Waiting { .. } => {
            Some(Note::new("waits for you", Tone::Warn))
        }
        Standing::Failing => Some(Note::new("tests fail", Tone::Danger)),
        Standing::Broken { .. } => {
            Some(Note::new("does not load", Tone::Danger))
        }
    };
    CatalogEntry {
        name: entry.name.clone(),
        description: entry.description.clone(),
        group: tau_ui_plugin::Group::Yours,
        seams,
        note,
    }
}
