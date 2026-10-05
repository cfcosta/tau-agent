//! tau-skills's host half (ADR 0030).

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, Seam};

use crate::{Skills, SkillsUi, ui::link};

/// What the plugin keeps on the host: where the skills are.
#[derive(Debug, Clone)]
pub struct Host {
    /// The person's skills.
    pub dir: Option<std::path::PathBuf>,
    /// The skills tau ships.
    pub builtin: std::path::PathBuf,
}

impl tau_ui_plugin::PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self {
            dir: cx
                .services
                .get::<crate::SkillsDir>()
                .map(|dir| dir.0.clone()),
            builtin: cx.dir.join(crate::BUILTIN_DIR),
        })
    }
}

/// tau-skills on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkillsHost;

impl SkillsHost {
    fn skills(host: &Host) -> Skills {
        crate::scan::scan_all(host.dir.as_deref(), &host.builtin)
    }
}

impl HostHalf for SkillsHost {
    type Plugin = SkillsUi;
    type Host = Host;

    /// Reads the folder as the run starts; with no skills, the run gets
    /// neither the list nor the tool.
    async fn agent_plugins(
        &self,
        host: &Host,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let skills = Self::skills(host);
        if skills.found.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![Box::new(crate::host::SkillsPlugin::new(skills))])
    }

    async fn catalog(
        &self,
        host: &Host,
        cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        let found = self.data(host, cx).await.found.len();
        PluginInfo {
            description: match found {
                1 => "Instructions the agent loads when a task calls for \
                      them: 1 skill"
                    .into(),
                n => format!(
                    "Instructions the agent loads when a task calls for \
                     them: {n} skills"
                ),
            },
            seams: vec![Seam::Start, Seam::Tools],
            page: Some(link()),
            note: Some(tau_ui_plugin::Note::new(
                match found {
                    1 => "1 skill".to_owned(),
                    n => format!("{n} skills"),
                },
                tau_ui_kit::theme::Tone::Quiet,
            )),
            ..PluginInfo::default()
        }
    }

    async fn data(&self, host: &Host, _cx: &HostCx) -> Skills {
        Self::skills(host)
    }
}
