//! What onboarding shows: the repositories to clone from the forge and
//! how their clones are going. Plugins add their own steps
//! (`points::SETUP`): the provider's sign-in first, the forge's before
//! the repositories.
//!
//! The host fills it in as sign-ins and clones progress, through
//! [`Workspace::update_setup`](crate::Workspace::update_setup).

use serde::{Deserialize, Serialize};
use tau_ui_plugin::points::{SetupAt, SetupClone, SetupRepo};

/// The screens of onboarding, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStep {
    Welcome,
    Repos,
    /// A plugin's step (`points::SETUP`), before the repositories or
    /// after them as it says: `step` is the plugin's name for it.
    Plugin {
        plugin: &'static str,
        step: &'static str,
    },
    /// Clones, and the first run's prompt.
    Ready,
}

/// A stage of onboarding's step bar: its name, and the step it stands
/// for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    pub name: String,
    pub step: SetupStep,
}

impl Stage {
    /// tau's stages around the plugins' `steps`, each a step, what the
    /// bar calls it and where it sits: the plugins' model steps, the
    /// plugins' before the repositories, Repositories, the plugins'
    /// after them, then Ready; plugins' in the order given.
    pub fn all(
        steps: impl IntoIterator<Item = (SetupStep, String, SetupAt)>,
    ) -> Vec<Self> {
        let stage = |name: &str, step| Self {
            name: name.to_owned(),
            step,
        };
        let steps: Vec<_> = steps.into_iter().collect();
        let at = |place: SetupAt| {
            steps.iter().filter(move |(_, _, at)| *at == place).map(
                |(step, name, _)| Self {
                    name: name.clone(),
                    step: *step,
                },
            )
        };
        let mut stages: Vec<Self> = at(SetupAt::Model).collect();
        stages.extend(at(SetupAt::BeforeRepositories));
        stages.push(stage("Repositories", SetupStep::Repos));
        stages.extend(at(SetupAt::AfterRepositories));
        stages.push(stage("Ready", SetupStep::Ready));
        stages
    }
}

impl SetupStep {
    /// The stage the step bar marks among `stages`: none for the welcome,
    /// which comes before any. A plugin's step that is not among them
    /// marks the repositories', where it would follow.
    pub fn stage(self, stages: &[Stage]) -> Option<usize> {
        let of =
            |step: Self| stages.iter().position(|stage| stage.step == step);
        match self {
            Self::Welcome => None,
            Self::Plugin { .. } => of(self).or_else(|| of(Self::Repos)),
            step => of(step),
        }
    }

    /// The screen's name in the title bar and the phone's header; a
    /// plugin's step names itself (`Workspace::route_title`).
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Repos => "Pick repositories",
            Self::Plugin { .. } => "Set up",
            Self::Ready => "First run",
        }
    }
}

/// A repository on the forge the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChoice {
    /// Its full name on the forge: `owner/name`.
    pub name: String,
    pub description: String,
    pub branch: String,
    pub selected: bool,
    /// When it was last pushed to, as the forge writes it
    /// (`2026-10-04T21:13:07Z`): the list shows the newest first.
    #[serde(default)]
    pub pushed_at: String,
    /// The URL tau clones it from, as the forge gives it.
    #[serde(default)]
    pub remote: Option<String>,
}

/// How many repositories the picker shows before "Show more".
pub const SHOWN_REPOS: usize = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CloneState {
    /// How far the clone is, from 0 to 1, and what it is doing.
    Cloning {
        share: f32,
        detail: String,
    },
    Ready,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoClone {
    pub name: String,
    pub state: CloneState,
    /// tau's id for the repository, once its clone is listed.
    #[serde(default)]
    pub id: Option<tau_ui_plugin::RepoId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    pub repos: Vec<RepoChoice>,
    pub clones: Vec<RepoClone>,
    /// Where clones go, as the user reads it: `~/.local/share/tau/repos/`.
    pub storage: String,
    /// Where tokens are kept, as the user reads it.
    pub config: String,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            repos: Vec::new(),
            clones: Vec::new(),
            storage: "~/.local/share/tau/repos/".into(),
            config: "~/.config/tau".into(),
        }
    }
}

/// What the host learned while the user set up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SetupUpdate {
    /// The repositories on the forge the user can pick from.
    Repos(Vec<RepoChoice>),
    Clone(RepoClone),
}

impl Setup {
    pub fn update(&mut self, update: SetupUpdate) {
        match update {
            SetupUpdate::Repos(repos) => self.repos = repos,
            SetupUpdate::Clone(clone) => {
                match self.clones.iter_mut().find(|c| c.name == clone.name) {
                    Some(known) => *known = clone,
                    None => self.clones.push(clone),
                }
            }
        }
    }

    pub fn selected(&self) -> impl Iterator<Item = &RepoChoice> {
        self.repos.iter().filter(|repo| repo.selected)
    }

    /// The repositories picked so far, as plugins' setup steps see them:
    /// those picked, then any other being cloned, each with its clone's
    /// state.
    pub fn picked(&self) -> Vec<SetupRepo> {
        let clone_of =
            |name: &str| self.clones.iter().find(|clone| clone.name == name);
        let state = |clone: Option<&RepoClone>| match clone.map(|c| &c.state) {
            None => SetupClone::Picked,
            Some(CloneState::Cloning { share, .. }) => {
                SetupClone::Cloning(*share)
            }
            Some(CloneState::Ready) => SetupClone::Ready,
            Some(CloneState::Failed(why)) => SetupClone::Failed(why.clone()),
        };
        let picked = self.selected().map(|repo| {
            let clone = clone_of(&repo.name);
            SetupRepo {
                name: repo.name.clone(),
                id: clone.and_then(|clone| clone.id.clone()),
                remote: repo.remote.clone(),
                folder: None,
                clone: state(clone),
            }
        });
        let others = self
            .clones
            .iter()
            .filter(|clone| {
                !self.selected().any(|repo| repo.name == clone.name)
            })
            .map(|clone| SetupRepo {
                name: clone.name.clone(),
                id: clone.id.clone(),
                remote: self
                    .repos
                    .iter()
                    .find(|repo| repo.name == clone.name)
                    .and_then(|repo| repo.remote.clone()),
                folder: None,
                clone: state(Some(clone)),
            });
        picked.chain(others).collect()
    }

    pub fn toggle(&mut self, name: &str) {
        if let Some(repo) = self.repos.iter_mut().find(|r| r.name == name) {
            repo.selected = !repo.selected;
        }
    }

    /// Repositories whose name or description contains `filter`,
    /// ignoring case.
    pub fn matching<'a>(
        &'a self,
        filter: &'a str,
    ) -> impl Iterator<Item = &'a RepoChoice> {
        let filter = filter.trim().to_lowercase();
        self.repos.iter().filter(move |repo| {
            filter.is_empty()
                || repo.name.to_lowercase().contains(&filter)
                || repo.description.to_lowercase().contains(&filter)
        })
    }

    /// What the picker shows for `filter`: the first [`SHOWN_REPOS`]
    /// matches, and any picked one past them, unless `expanded`, which
    /// shows them all; and how many matches it leaves out.
    pub fn shown<'a>(
        &'a self,
        filter: &'a str,
        expanded: bool,
    ) -> (Vec<&'a RepoChoice>, usize) {
        let matching: Vec<&RepoChoice> = self.matching(filter).collect();
        if expanded {
            return (matching, 0);
        }
        let shown: Vec<&RepoChoice> = matching
            .iter()
            .enumerate()
            .filter(|(n, repo)| *n < SHOWN_REPOS || repo.selected)
            .map(|(_, repo)| *repo)
            .collect();
        let hidden = matching.len() - shown.len();
        (shown, hidden)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str, description: &str) -> RepoChoice {
        RepoChoice {
            name: name.into(),
            description: description.into(),
            branch: "main".into(),
            selected: false,
            pushed_at: String::new(),
            remote: None,
        }
    }

    /// The picker shows the first twenty matches and any picked one past
    /// them, in the list's order, and counts the rest; expanded, it shows
    /// every match.
    #[test]
    fn the_picker_shows_twenty_and_what_is_picked() {
        let mut setup = Setup {
            repos: (0..30).map(|n| repo(&format!("a/r{n:02}"), "")).collect(),
            ..Setup::default()
        };
        setup.toggle("a/r25");
        let (shown, hidden) = setup.shown("", false);
        let names: Vec<&str> = shown.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names.len(), SHOWN_REPOS + 1);
        assert_eq!(names[SHOWN_REPOS - 1], "a/r19");
        assert_eq!(names[SHOWN_REPOS], "a/r25");
        assert_eq!(hidden, 9);
        assert_eq!(setup.shown("", true), (setup.repos.iter().collect(), 0));
        let (shown, hidden) = setup.shown("r1", false);
        assert_eq!((shown.len(), hidden), (10, 0));
    }

    #[test]
    fn filters_and_toggles_repositories() {
        let mut setup = Setup {
            repos: vec![repo("a/tau", "agents"), repo("a/docs", "Search")],
            ..Setup::default()
        };
        let found: Vec<_> =
            setup.matching("SEARCH").map(|r| r.name.as_str()).collect();
        assert_eq!(found, ["a/docs"]);
        assert_eq!(setup.matching(" ").count(), 2);
        setup.toggle("a/tau");
        assert_eq!(setup.selected().count(), 1);
        setup.toggle("a/tau");
        assert_eq!(setup.selected().count(), 0);
    }

    /// A clone's update replaces the one of its name, in its place, and
    /// leaves the others; a new name joins at the end.
    #[test]
    fn clone_updates_replace_by_name() {
        let mut setup = Setup::default();
        let cloning = |name: &str, share| RepoClone {
            name: name.into(),
            state: CloneState::Cloning {
                share,
                detail: String::new(),
            },
            id: None,
        };
        setup.update(SetupUpdate::Clone(cloning("a/tau", 0.1)));
        setup.update(SetupUpdate::Clone(cloning("a/docs", 0.2)));
        setup.update(SetupUpdate::Clone(cloning("a/tau", 0.5)));
        assert_eq!(
            setup.clones,
            [cloning("a/tau", 0.5), cloning("a/docs", 0.2)]
        );
        setup.update(SetupUpdate::Clone(cloning("a/docs", 0.9)));
        setup.update(SetupUpdate::Clone(cloning("a/web", 0.0)));
        assert_eq!(
            setup.clones,
            [
                cloning("a/tau", 0.5),
                cloning("a/docs", 0.9),
                cloning("a/web", 0.0)
            ]
        );
    }

    /// Plugins' steps sit first, before the repositories or between
    /// them and Ready, as each says, in the order given; each step marks
    /// its own stage, and the welcome none.
    #[test]
    fn plugins_steps_sit_around_the_repositories() {
        let step = |step| SetupStep::Plugin { plugin: "p", step };
        let (model, sign_in, one, two) =
            (step("model"), step("sign-in"), step("one"), step("two"));
        let stages = Stage::all([
            (one, "One".to_owned(), SetupAt::AfterRepositories),
            (sign_in, "Sign in".to_owned(), SetupAt::BeforeRepositories),
            (model, "Model".to_owned(), SetupAt::Model),
            (two, "Two".to_owned(), SetupAt::AfterRepositories),
        ]);
        let names: Vec<&str> =
            stages.iter().map(|stage| stage.name.as_str()).collect();
        assert_eq!(
            names,
            ["Model", "Sign in", "Repositories", "One", "Two", "Ready"]
        );
        assert_eq!(sign_in.stage(&stages), Some(1));
        assert_eq!(one.stage(&stages), Some(3));
        assert_eq!(SetupStep::Ready.stage(&stages), Some(5));
        assert_eq!(model.stage(&stages), Some(0));
        assert_eq!(SetupStep::Welcome.stage(&stages), None);
        let gone = step("gone");
        assert_eq!(gone.stage(&stages), Some(2), "where it would follow");
        assert_eq!(Stage::all([]).len(), 2);
    }

    /// What plugins' steps see: the picked repositories with their
    /// remotes, and each clone's state and id once it has one.
    #[test]
    fn picked_repositories_carry_their_clone() {
        let mut setup = Setup {
            repos: vec![
                RepoChoice {
                    remote: Some("https://github.com/a/tau.git".into()),
                    ..repo("a/tau", "")
                },
                repo("a/docs", ""),
            ],
            ..Setup::default()
        };
        setup.toggle("a/tau");
        setup.toggle("a/docs");
        setup.update(SetupUpdate::Clone(RepoClone {
            name: "a/tau".into(),
            state: CloneState::Ready,
            id: Some(tau_ui_plugin::RepoId("id-tau".into())),
        }));
        assert_eq!(
            setup.picked(),
            [
                SetupRepo {
                    name: "a/tau".into(),
                    id: Some(tau_ui_plugin::RepoId("id-tau".into())),
                    remote: Some("https://github.com/a/tau.git".into()),
                    folder: None,
                    clone: SetupClone::Ready,
                },
                SetupRepo {
                    name: "a/docs".into(),
                    id: None,
                    remote: None,
                    folder: None,
                    clone: SetupClone::Picked,
                },
            ]
        );
    }
}
