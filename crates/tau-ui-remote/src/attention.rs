//! Whether a chat needs the person, as its row in the sidebar and on a
//! phone says it: working, asking, ready to land, would conflict,
//! interrupted, landed.
//!
//! [`Attention::of`] is the one place that decides it, from a run's
//! [`Facts`]. The workspace gathers the facts
//! ([`Workspace::attention`]): the run's status and turn, what a plugin
//! holds it for ([`points::ASKS`]), and the landing forecast the host
//! works out in the background ([`RunView::forecast`]). All of them
//! travel in a snapshot, so a phone shows the same states.

use gpui::App;
use serde::{Deserialize, Serialize};
use tau_agent::event::StopReason;
use tau_ui_plugin::points;

use crate::{
    view::{INTERRUPTED, Item, RunStatus, RunView},
    workspace::Workspace,
};

/// What landing a finished fork on its parent would do, as the host
/// last worked it out, in the background.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forecast {
    /// The changes it would land.
    pub changes: usize,
    /// The files that would conflict.
    pub conflicts: Vec<String>,
}

impl From<&tau_vcs::Landing> for Forecast {
    fn from(landing: &tau_vcs::Landing) -> Self {
        Self {
            changes: landing.changes.len(),
            conflicts: landing.conflicts.clone(),
        }
    }
}

/// Where a run is, as far as attention goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    /// Going: planning, or in a turn.
    #[default]
    Live,
    /// It stopped: done, cancelled, or at a limit. It waits for the
    /// person's next message, at their pace.
    Stopped,
    /// It stopped with an error.
    Failed,
    /// It was going when tau closed.
    Interrupted,
}

impl Status {
    pub fn of(status: &RunStatus) -> Self {
        match status {
            RunStatus::Planning | RunStatus::Running => Self::Live,
            RunStatus::Finished(StopReason::Error(error))
                if error == INTERRUPTED =>
            {
                Self::Interrupted
            }
            RunStatus::Interrupted => Self::Interrupted,
            RunStatus::Finished(StopReason::Error(_)) => Self::Failed,
            RunStatus::Finished(_) => Self::Stopped,
        }
    }
}

/// What [`Attention::of`] reads of a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub status: Status,
    /// The turn it is in.
    pub turn: u32,
    /// What a plugin holds it for the person to answer
    /// ([`points::ASKS`]).
    pub asks: Option<String>,
    /// What landing it on its parent would do, when the host worked it
    /// out.
    pub forecast: Option<Forecast>,
    /// It landed on its parent, which closed it.
    pub landed: bool,
}

/// Whether a run needs the person, and why. Rows never move for it:
/// the sidebar keeps its order, and only the row's icon and line change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    /// Going, in this turn.
    Working { turn: u32 },
    /// Waiting on the person to answer this.
    Asks { question: String },
    /// A finished fork whose changes would land cleanly.
    ReadyToLand { changes: usize },
    /// A finished fork whose landing would conflict in these files.
    WouldConflict { files: Vec<String> },
    /// It was going when tau closed.
    Interrupted,
    /// It stopped with an error.
    Failed,
    /// It landed on its parent and closed.
    Landed,
    /// Nothing: it waits for the person's next message, at their pace.
    Idle,
}

impl Attention {
    /// The one place a run's attention is decided. In order:
    ///
    /// 1. A landed run is landed, whatever else it says.
    /// 2. A live run asks, when a plugin holds it for an answer, and
    ///    otherwise works.
    /// 3. A run tau's closing interrupted, or that failed, says so: its
    ///    work did not finish, so what landing it would do is beside the
    ///    point.
    /// 4. A stopped fork says what landing it would do: conflict, or
    ///    land its changes. With nothing to land it is idle.
    ///
    /// The landing queue's `Queued` belongs before step 4, and the main
    /// chat's "conflicts on main" after step 2: each a variant here and
    /// a fact in [`Facts`].
    pub fn of(facts: &Facts) -> Self {
        if facts.landed {
            return Self::Landed;
        }
        match facts.status {
            Status::Live => {
                return match &facts.asks {
                    Some(question) => Self::Asks {
                        question: question.clone(),
                    },
                    None => Self::Working { turn: facts.turn },
                };
            }
            Status::Interrupted => return Self::Interrupted,
            Status::Failed => return Self::Failed,
            Status::Stopped => {}
        }
        match &facts.forecast {
            Some(forecast) if !forecast.conflicts.is_empty() => {
                Self::WouldConflict {
                    files: forecast.conflicts.clone(),
                }
            }
            Some(forecast) if forecast.changes > 0 => Self::ReadyToLand {
                changes: forecast.changes,
            },
            _ => Self::Idle,
        }
    }

    /// Whether the repository's "N need you" counts it: it asks, would
    /// conflict, or is ready to land.
    pub fn needs_you(&self) -> bool {
        matches!(
            self,
            Self::Asks { .. }
                | Self::WouldConflict { .. }
                | Self::ReadyToLand { .. }
        )
    }

    /// The line under the run's title, if it has one.
    pub fn line(&self) -> Option<String> {
        Some(match self {
            Self::Working { turn } => format!("Working · turn {turn}"),
            Self::Asks { .. } => "Asks you a question".to_owned(),
            Self::ReadyToLand { changes } => {
                format!("Ready to land · {}", count(*changes, "change"))
            }
            Self::WouldConflict { files } => {
                format!("Would conflict in {}", count(files.len(), "file"))
            }
            Self::Interrupted => "Interrupted · tau closed".to_owned(),
            Self::Failed | Self::Landed | Self::Idle => return None,
        })
    }
}

/// `1 change`, `2 changes`.
pub fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

impl Workspace {
    /// What `run`'s row says about whether it needs the person.
    pub fn attention(&self, run: &RunView, cx: &mut App) -> Attention {
        Attention::of(&self.facts(run, cx))
    }

    /// What [`Attention::of`] reads of `run`.
    pub fn facts(&self, run: &RunView, cx: &mut App) -> Facts {
        let asks = self
            .contributions(points::ASKS, &points::AtRun { run: run.info() }, cx)
            .into_iter()
            .next();
        Facts {
            status: Status::of(&run.status),
            turn: run.turn,
            asks,
            forecast: run.forecast.clone(),
            landed: self.is_closed(&run.id) && self.has_landed(run),
        }
    }

    /// Whether `run`'s changes landed on its parent: the parent's chat
    /// has the landing's card.
    fn has_landed(&self, run: &RunView) -> bool {
        let Some(parent) = run.origin.parent().and_then(|id| self.run(id))
        else {
            return false;
        };
        parent.items.iter().any(
            |item| matches!(item, Item::Landed(card) if card.from == run.id),
        )
    }

    /// How many of `repo`'s open chats need the person: they ask, would
    /// conflict, or are ready to land.
    pub fn need_you(&self, repo: &str, cx: &mut App) -> usize {
        self.runs
            .iter()
            .filter(|run| self.repo_of(run) == repo && !self.is_closed(&run.id))
            .filter(|run| self.attention(run, cx).needs_you())
            .count()
    }
}

impl Workspace {
    /// `repo`'s forks that finished and are still open, newest first:
    /// those the host works out a landing forecast for.
    pub fn finished_forks(&self, repo: &str) -> Vec<tau_agent::tool::RunId> {
        self.runs
            .iter()
            .filter(|run| {
                matches!(run.origin, crate::view::Origin::Fork { .. })
                    && !run.status.is_live()
                    && self.repo_of(run) == repo
                    && !self.is_closed(&run.id)
            })
            .map(|run| run.id.clone())
            .collect()
    }
}
