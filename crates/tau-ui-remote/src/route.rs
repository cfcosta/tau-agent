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
    /// A repository's page: its runs, with tabs for what plugins keep
    /// for it.
    Repo(String),
    /// A plugin's page (ADR 0017).
    Plugin {
        plugin: String,
        page: String,
        params: std::collections::BTreeMap<String, String>,
    },
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
    /// tau-agents' page `page`: the Agents page, or the New agent form
    /// (ADR 0037).
    pub fn agents(page: &str) -> Self {
        Self::Plugin {
            plugin: tau_agents::NAME.to_owned(),
            page: page.to_owned(),
            params: Default::default(),
        }
    }

    /// The run a screen is about, if any.
    pub fn run(&self) -> Option<&RunId> {
        match self {
            Self::Run(run) | Self::Plan(run) | Self::PullRequest(run) => {
                Some(run)
            }
            Self::Compare { main, .. } => Some(main),
            _ => None,
        }
    }

    /// The repository a screen is about, if it names one.
    pub fn repo(&self) -> Option<&str> {
        match self {
            Self::Plugin { params, .. } => {
                params.get("repo").map(String::as_str)
            }
            Self::Repo(repo) => Some(repo),
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
            Self::Setup(step) => step.title(),
            Self::Pair(step) => step.title(),
            Self::PullRequest(_) => "Pull request",
            Self::Models => "Models",
            Self::Phones => "Phones",
            Self::Repo(_) => "Repository",
            // The page names itself; see `Workspace::route_title`.
            Self::Plugin { .. } => "",
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

    /// Every screen knows its run, its repository, its tab, and whether
    /// it is top-level or focused: the table lists each kind of screen.
    /// Each tab leads to a top-level screen under itself.
    #[test]
    fn every_screen_knows_its_run_repository_and_tab() {
        // (screen, run, repository, tab, top-level, focused)
        type Row<'a> =
            (Route, Option<&'a RunId>, Option<&'a str>, Tab, bool, bool);
        let (main, fork) = (RunId(Arc::from("main")), RunId(Arc::from("fork")));
        let plugin = |params: &[(&str, &str)]| Route::Plugin {
            plugin: "tau-memory".into(),
            page: "notes".into(),
            params: params
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
        };
        let table: Vec<Row> = vec![
            (Route::Home, None, None, Tab::Runs, true, false),
            (
                Route::Run(main.clone()),
                Some(&main),
                None,
                Tab::Runs,
                false,
                false,
            ),
            (Route::NewRun, None, None, Tab::Runs, false, false),
            (
                Route::Compare {
                    main: main.clone(),
                    fork: fork.clone(),
                },
                Some(&main),
                None,
                Tab::Runs,
                false,
                false,
            ),
            (Route::History, None, None, Tab::History, true, false),
            (Route::Plugins, None, None, Tab::Plugins, true, false),
            (
                Route::Plan(fork.clone()),
                Some(&fork),
                None,
                Tab::Runs,
                false,
                false,
            ),
            (
                Route::Setup(SetupStep::Welcome),
                None,
                None,
                Tab::Runs,
                false,
                true,
            ),
            (
                Route::Pair(PairStep::Welcome),
                None,
                None,
                Tab::Runs,
                false,
                true,
            ),
            (
                Route::PullRequest(fork.clone()),
                Some(&fork),
                None,
                Tab::Runs,
                false,
                true,
            ),
            (Route::Models, None, None, Tab::Models, true, false),
            (Route::Phones, None, None, Tab::Models, false, false),
            (
                Route::Repo("docbert".into()),
                None,
                Some("docbert"),
                Tab::Runs,
                false,
                false,
            ),
            (
                plugin(&[("repo", "docbert")]),
                None,
                Some("docbert"),
                Tab::Runs,
                false,
                false,
            ),
            (
                plugin(&[("run", "main")]),
                None,
                None,
                Tab::Runs,
                false,
                false,
            ),
        ];
        for (route, run, repo, tab, top, focused) in &table {
            assert_eq!(
                (
                    route.run(),
                    route.repo(),
                    route.tab(),
                    route.is_top_level(),
                    route.is_focused()
                ),
                (*run, *repo, *tab, *top, *focused),
                "{route:?}"
            );
        }
        for tab in Tab::ALL {
            assert!(tab.route().is_top_level(), "{tab:?}");
            assert_eq!(tab.route().tab(), tab);
        }
    }
}
