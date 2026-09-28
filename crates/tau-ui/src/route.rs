//! The screens the workspace can show, and how they read in the title
//! bar and on the phone's tab bar.

use tau_agent::tool::RunId;

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
    Memory {
        note: Option<String>,
    },
    Constitution {
        rule: Option<String>,
    },
    /// A run's pruning ledger.
    Ledger(RunId),
}

/// The phone's bottom tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Runs,
    Memory,
    History,
    Plugins,
}

impl Tab {
    pub const ALL: [Self; 4] =
        [Self::Runs, Self::Memory, Self::History, Self::Plugins];

    pub fn label(self) -> &'static str {
        match self {
            Self::Runs => "Runs",
            Self::Memory => "Memory",
            Self::History => "History",
            Self::Plugins => "Plugins",
        }
    }

    pub fn route(self) -> Route {
        match self {
            Self::Runs => Route::Home,
            Self::Memory => Route::Memory { note: None },
            Self::History => Route::History,
            Self::Plugins => Route::Plugins,
        }
    }
}

impl Route {
    /// The run a screen is about, if any.
    pub fn run(&self) -> Option<&RunId> {
        match self {
            Self::Run(run) | Self::Plan(run) | Self::Ledger(run) => Some(run),
            Self::Compare { main, .. } => Some(main),
            _ => None,
        }
    }

    /// The tab a screen belongs under.
    pub fn tab(&self) -> Tab {
        match self {
            Self::Memory { .. } => Tab::Memory,
            Self::History => Tab::History,
            Self::Plugins | Self::Constitution { .. } => Tab::Plugins,
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
        }
    }

    /// Top-level screens get the phone's tab bar.
    pub fn is_top_level(&self) -> bool {
        matches!(
            self,
            Self::Home
                | Self::History
                | Self::Plugins
                | Self::Memory { note: None }
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
        assert!(
            !Route::Memory {
                note: Some("n".into())
            }
            .is_top_level()
        );
    }
}
