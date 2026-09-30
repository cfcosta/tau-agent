//! What onboarding shows: the GitHub sign-in, the model, the
//! repositories to clone and how their clones are going.
//!
//! The host fills it in as sign-ins and clones progress, through
//! [`Workspace::update_setup`](crate::Workspace::update_setup).

use serde::{Deserialize, Serialize};

/// The screens of onboarding, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SetupStep {
    Welcome,
    GitHub,
    /// A personal access token instead of the device flow.
    Token,
    Model,
    Repos,
    /// Clones, and the first run's prompt.
    Ready,
}

impl SetupStep {
    /// The stage the progress bar marks: GitHub, Model, Repositories or
    /// Ready. The welcome comes before any.
    pub fn stage(self) -> usize {
        match self {
            Self::Welcome | Self::GitHub | Self::Token => 0,
            Self::Model => 1,
            Self::Repos => 2,
            Self::Ready => 3,
        }
    }

    pub const STAGES: [&str; 4] = ["GitHub", "Model", "Repositories", "Ready"];

    /// The screen's name in the title bar and the phone's header.
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::GitHub => "Sign in with GitHub",
            Self::Token => "Personal access token",
            Self::Model => "Connect a model",
            Self::Repos => "Pick repositories",
            Self::Ready => "First run",
        }
    }
}

/// A code to enter on another page, for a device sign-in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceCode {
    pub code: String,
    /// Where to enter it, without the scheme: `github.com/login/device`.
    pub url: String,
    /// How long it stays valid, in words.
    pub expires: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitHub {
    #[default]
    SignedOut,
    /// Waiting for the user to enter the code and approve.
    Waiting(DeviceCode),
    /// A token is being checked.
    Checking,
    SignedIn {
        user: String,
    },
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelAccess {
    #[default]
    None,
    /// A ChatGPT sign-in is open in the browser at `url`, once known;
    /// its redirect can also be pasted.
    SigningIn {
        url: Option<String>,
    },
    /// Runs can start. `label` reads like `gpt-5.5 · ChatGPT plan`.
    Connected {
        label: String,
    },
    /// Signed in to ChatGPT as `account`, but without plan usage: runs
    /// cannot use the plan until it is enabled.
    PlanDisabled {
        account: String,
    },
    /// Signed in, but OpenAI says plan use is not available to this
    /// account or workspace (`subscription_sharing_user_not_eligible`,
    /// `tau_ai`'s `Recovery::Restricted`). Signing in again with the same
    /// account will not help. `detail` is the refusal as OpenAI sent it:
    /// status, code and request id.
    NotEligible {
        account: String,
        detail: String,
    },
    Failed(String),
}

/// A repository the user can pick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoChoice {
    /// `owner/name`.
    pub name: String,
    pub description: String,
    pub branch: String,
    pub selected: bool,
}

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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    pub github: GitHub,
    pub model: ModelAccess,
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
            github: GitHub::default(),
            model: ModelAccess::default(),
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
    GitHub(GitHub),
    Model(ModelAccess),
    /// The repositories the user can pick from.
    Repos(Vec<RepoChoice>),
    Clone(RepoClone),
}

impl Setup {
    pub fn update(&mut self, update: SetupUpdate) {
        match update {
            SetupUpdate::GitHub(github) => self.github = github,
            SetupUpdate::Model(model) => self.model = model,
            SetupUpdate::Repos(repos) => self.repos = repos,
            SetupUpdate::Clone(clone) => {
                match self.clones.iter_mut().find(|c| c.name == clone.name) {
                    Some(known) => *known = clone,
                    None => self.clones.push(clone),
                }
            }
        }
    }

    pub fn user(&self) -> Option<&str> {
        match &self.github {
            GitHub::SignedIn { user } => Some(user),
            _ => None,
        }
    }

    pub fn selected(&self) -> impl Iterator<Item = &RepoChoice> {
        self.repos.iter().filter(|repo| repo.selected)
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

    /// The model label for the first run's chips.
    pub fn model_label(&self) -> Option<&str> {
        match &self.model {
            ModelAccess::Connected { label } => Some(label),
            _ => None,
        }
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
        }
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

    #[test]
    fn clone_updates_replace_by_name() {
        let mut setup = Setup::default();
        let cloning = |share| RepoClone {
            name: "a/tau".into(),
            state: CloneState::Cloning {
                share,
                detail: String::new(),
            },
        };
        setup.update(SetupUpdate::Clone(cloning(0.1)));
        setup.update(SetupUpdate::Clone(cloning(0.5)));
        assert_eq!(setup.clones, [cloning(0.5)]);
    }

    #[test]
    fn stages_follow_the_steps() {
        assert_eq!(SetupStep::Token.stage(), SetupStep::GitHub.stage());
        assert!(SetupStep::Model.stage() < SetupStep::Ready.stage());
    }
}
