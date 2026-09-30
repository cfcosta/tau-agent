//! The screens the workspace can show, and how they read in the title
//! bar and on the phone's tab bar.

use tau_agent::tool::RunId;

use crate::{pairing::PairStep, setup::SetupStep};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The run list. On a desktop, the sidebar is the list, so this shows
    /// the current run.
    Home,
    Run(RunId),
    NewRun,
    Compare {
        main: RunId,
        fork: RunId,
    },
    History,
    Plugins,
    /// What each plugin's `start` decided for a run.
    Plan(RunId),
    /// A repository's notes.
    Memory {
        repo: String,
        note: Option<String>,
    },
    /// A repository's rules.
    Constitution {
        repo: String,
        rule: Option<String>,
    },
    /// A run's pruning ledger.
    Ledger(RunId),
    /// Onboarding: GitHub, a model, repositories, the first run.
    Setup(SetupStep),
    /// A phone pairing with the tau on a computer, or failing to reach
    /// it.
    Pair(PairStep),
    /// A pull request from a run: the draft, then the opened one.
    PullRequest(RunId),
    /// Model settings: defaults, what the picker shows, what to ask about.
    Models,
    /// Allow phones, pair one, and see those paired.
    Phones,
}

/// The phone's bottom tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Runs, grouped by repository, with each one's memory and rules.
    Runs,
    History,
    Plugins,
    Models,
}

impl Tab {
    pub const ALL: [Self; 4] =
        [Self::Runs, Self::History, Self::Plugins, Self::Models];

    pub fn label(self) -> &'static str {
        match self {
            Self::Runs => "Runs",
            Self::History => "History",
            Self::Plugins => "Plugins",
            Self::Models => "Models",
        }
    }

    pub fn route(self) -> Route {
        match self {
            Self::Runs => Route::Home,
            Self::History => Route::History,
            Self::Plugins => Route::Plugins,
            Self::Models => Route::Models,
        }
    }
}

impl Route {
    /// The run a screen is about, if any.
    pub fn run(&self) -> Option<&RunId> {
        match self {
            Self::Run(run)
            | Self::Plan(run)
            | Self::Ledger(run)
            | Self::PullRequest(run) => Some(run),
            Self::Compare { main, .. } => Some(main),
            _ => None,
        }
    }

    /// The repository a screen is about, if it names one.
    pub fn repo(&self) -> Option<&str> {
        match self {
            Self::Memory { repo, .. } | Self::Constitution { repo, .. } => {
                Some(repo)
            }
            _ => None,
        }
    }

    /// The tab a screen belongs under.
    pub fn tab(&self) -> Tab {
        match self {
            Self::History => Tab::History,
            Self::Plugins => Tab::Plugins,
            Self::Models | Self::Phones => Tab::Models,
            _ => Tab::Runs,
        }
    }

    /// The screen's name in the title bar.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Home | Self::Run(_) => "Run",
            Self::NewRun => "New run",
            Self::Compare { .. } => "Compare forks",
            Self::History => "History",
            Self::Plugins => "Plugins",
            Self::Plan(_) => "Run plan",
            Self::Memory { .. } => "Memory",
            Self::Constitution { .. } => "Constitution",
            Self::Ledger(_) => "Context ledger",
            Self::Setup(step) => step.title(),
            Self::Pair(step) => step.title(),
            Self::PullRequest(_) => "Pull request",
            Self::Models => "Models",
            Self::Phones => "Phones",
        }
    }

    /// Screens that take the whole window, without the sidebar: a task
    /// to finish before going back to the runs.
    pub fn is_focused(&self) -> bool {
        matches!(self, Self::Setup(_) | Self::Pair(_) | Self::PullRequest(_))
    }

    /// Top-level screens get the phone's tab bar.
    pub fn is_top_level(&self) -> bool {
        matches!(
            self,
            Self::Home | Self::History | Self::Plugins | Self::Models
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn every_tab_leads_to_a_top_level_screen() {
        for tab in Tab::ALL {
            assert!(tab.route().is_top_level(), "{tab:?}");
            assert_eq!(tab.route().tab(), tab);
        }
    }

    #[test]
    fn run_screens_know_their_run() {
        let run = RunId(Arc::from("r"));
        assert_eq!(Route::Ledger(run.clone()).run(), Some(&run));
        assert_eq!(Route::Plan(run.clone()).tab(), Tab::Runs);
        let memory = Route::Memory {
            repo: "docbert".into(),
            note: None,
        };
        assert!(!memory.is_top_level());
        assert_eq!(memory.tab(), Tab::Runs);
        assert_eq!(memory.repo(), Some("docbert"));
    }
}
