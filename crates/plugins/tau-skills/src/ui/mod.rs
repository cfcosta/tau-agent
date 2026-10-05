//! The plugin, with its UI: the Skills screen and its place in the
//! sidebar, the card of a `skill` call, and each skill as `/name` in the
//! composer, on the computer and on phones alike.

pub mod card;
pub mod page;

use tau_ui_kit::{assets::Icon, theme::Tone};
use tau_ui_plugin::{
    Link,
    ListedCommand,
    Manifest,
    NavEntry,
    Page,
    UiPlugin,
    ViewCx,
    points::{self, AtApp},
};

use crate::{NAME, Skills};

/// The plugin, as the registry holds it.
pub struct SkillsUi;

impl UiPlugin for SkillsUi {
    type State = ();
    /// The skills, as the folder holds them now.
    type Data = Skills;
    type RepoData = ();
    type Settings = ();
    type Ui = ();

    fn name(&self) -> &'static str {
        NAME
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
