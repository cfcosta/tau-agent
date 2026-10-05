//! The plugin, with its UI: the Skills screen and its place in the
//! sidebar, the card of a `skill` call, and each skill as `/name` in the
//! composer, on the computer and on phones alike.

pub mod card;
pub mod page;

use tau_agent::plugin::Plugin;
use tau_ui_kit::{assets::Icon, theme::Tone};
use tau_ui_plugin::{
    HostCx,
    Link,
    ListedCommand,
    Manifest,
    NavEntry,
    Page,
    PluginInfo,
    RunCtx,
    Seam,
    UiPlugin,
    ViewCx,
    points::{self, AtApp},
};

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
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
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
    async fn agent_plugins(
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
            ..PluginInfo::default()
        }
    }

    async fn data(&self, host: &Host, _cx: &HostCx) -> Skills {
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
            .page(Page::new("skills", page::render).title(page::title))
            .contribute_at(points::SIDEBAR, 20, sidebar)
            .contribute(points::CARD, card::card)
            .listed_commands(commands, run_command)
    }
}

/// The Skills screen.
pub fn link() -> Link {
    Link::page("skills")
}

/// The sidebar's Skills entry: how many there are, and how many folders
/// could not be read as one.
fn sidebar(_: &AtApp, view: &mut ViewCx<'_, SkillsUi>) -> Option<NavEntry> {
    let skills = view.data;
    if skills.found.is_empty() && skills.problems.is_empty() {
        return None;
    }
    let problems = skills.problems.len();
    Some(
        NavEntry::new("Skills", Icon::Skill, link())
            .detail(match skills.found.len() {
                1 => "1 skill".to_owned(),
                n => format!("{n} skills"),
            })
            .badge(
                (problems > 0).then(|| (problems.to_string(), Tone::Danger)),
            ),
    )
}

/// Each skill as `/name` in the composer's menu, with what to do after
/// it.
pub fn commands(skills: &Skills, _: Option<&()>) -> Vec<ListedCommand> {
    skills
        .found
        .iter()
        .map(|skill| ListedCommand {
            name: skill.name.clone(),
            hint: skill.description.clone(),
            args: "what to do".to_owned(),
            icon: Icon::Skill,
        })
        .collect()
}

/// Sends `/name` and what follows it as the message: the instructions
/// ask the model to load the skill a message starts with.
fn run_command(
    name: &str,
    args: &str,
    _repo: Option<&str>,
    view: &mut ViewCx<'_, SkillsUi>,
) {
    let text = if args.is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {args}")
    };
    let run = view.run.map(|run| run.id.clone());
    view.handle.send(run.as_ref(), text, view.cx);
}
