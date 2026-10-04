//! The plugin, with its UI. Its screens come with the design: for now it
//! lists the skills on the Plugins screen and hands each run its agent
//! plugin.

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{HostCx, Manifest, PluginInfo, RunCtx, Seam, UiPlugin};

use crate::{NAME, Skills};

/// The plugin, as the registry holds it.
pub struct SkillsUi;

/// What the plugin keeps on the host: where the skills are.
#[cfg(feature = "host")]
#[derive(Debug, Clone)]
pub struct Host {
    pub dir: Option<std::path::PathBuf>,
}

#[cfg(feature = "host")]
impl tau_ui_plugin::PluginHost for Host {
    fn new(cx: &HostCx) -> anyhow::Result<Self> {
        Ok(Self {
            dir: cx
                .services
                .get::<crate::SkillsDir>()
                .map(|dir| dir.0.clone()),
        })
    }
}

#[cfg(not(feature = "host"))]
pub type Host = ();

impl SkillsUi {
    #[cfg(feature = "host")]
    fn skills(host: &Host) -> Skills {
        host.dir
            .as_deref()
            .map(crate::scan::scan)
            .unwrap_or_default()
    }
}

impl UiPlugin for SkillsUi {
    type State = ();
    /// The skills, as the folder holds them now.
    type Data = Skills;
    type RepoData = ();
    type Settings = ();
    type Host = Host;
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
    }

    /// Reads the folder as the run starts; with no skills, the run gets
    /// neither the list nor the tool.
    fn agent_plugins(
        &self,
        host: &Host,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        #[cfg(feature = "host")]
        {
            let skills = Self::skills(host);
            if skills.found.is_empty() {
                return Ok(Vec::new());
            }
            Ok(vec![Box::new(crate::host::SkillsPlugin::new(skills))])
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = host;
            Ok(Vec::new())
        }
    }

    fn catalog(&self, host: &Host, cx: &HostCx, _settings: &()) -> PluginInfo {
        let found = self.data(host, cx).found.len();
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
            ..PluginInfo::default()
        }
    }

    fn data(&self, host: &Host, _cx: &HostCx) -> Skills {
        #[cfg(feature = "host")]
        {
            Self::skills(host)
        }
        #[cfg(not(feature = "host"))]
        {
            let _ = host;
            Skills::default()
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}
