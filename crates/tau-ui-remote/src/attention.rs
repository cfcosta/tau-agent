//! Whether a chat needs the person, as its row in the sidebar and on a
//! phone says it: working, asking, conflicts on main, queued to land,
//! ready to land, would conflict, interrupted, landed, dropped.
//!
//! [`Attention::of`] is the one place that decides it, from a run's
//! [`Facts`]. The workspace gathers the facts
//! ([`Workspace::attention`]): the run's status and turn, what a plugin
//! holds it for ([`points::ASKS`]), the landing forecast the host works
//! out in the background ([`RunView::forecast`]), how it ended for good
//! ([`RunView::ending`]), its place in its main chat's landing queue
//! ([`RunView::landing_queue`]), and, for a main chat, the conflicts a
//! turn left on it ([`RunView::main_conflicts`]). All of them travel in a snapshot, so a
//! phone shows the same states.

use gpui::App;
use serde::{Deserialize, Serialize};
use tau_agent::event::StopReason;
use tau_ui_plugin::points;

use crate::{
    view::{Ending, RunStatus, RunView},
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
    /// How it ended for good: landed on its parent, or dropped.
    pub ending: Option<Ending>,
    /// Where it waits in its main chat's landing queue.
    pub queued: Option<Queued>,
    /// A main chat's: the files a turn left in conflict on it, while it
    /// is marked so.
    pub main_conflicts: Option<Vec<String>>,
}

/// A chat's place in its main chat's landing queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Queued {
    /// From 1: the first lands next.
    pub position: usize,
    /// It would conflict in a file the person did not confirm, so it
    /// waits for them ([`crate::queue::Waiting::needs_confirmation`]).
    pub needs_confirmation: bool,
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
    /// A main chat a turn left conflicts on, in these files: nothing
    /// lands on it and no chat forks it until they are resolved.
    ConflictsOnMain { files: Vec<String> },
    /// Waiting in its main chat's landing queue.
    Queued(Queued),
    /// It was going when tau closed.
    Interrupted,
    /// It stopped with an error.
    Failed,
    /// It landed on its parent and closed.
    Landed,
    /// It was dropped: its changes abandoned, and it closed.
    Dropped,
    /// Nothing: it waits for the person's next message, at their pace.
    Idle,
}

impl Attention {
    /// The one place a run's attention is decided. In order:
    ///
    /// 1. A run that landed or was dropped says so, whatever else it
    ///    says: it takes no more messages.
    /// 2. A live run asks, when a plugin holds it for an answer, and
    ///    otherwise works.
    /// 3. A main chat a turn left conflicts on says so.
    /// 4. A run tau's closing interrupted, or that failed, says so: its
    ///    work did not finish, so what landing it would do is beside the
    ///    point.
    /// 5. A chat queued to land says where it waits.
    /// 6. A stopped fork says what landing it would do: conflict, or
    ///    land its changes. With nothing to land it is idle.
    pub fn of(facts: &Facts) -> Self {
        match facts.ending {
            Some(Ending::Landed { .. }) => return Self::Landed,
            Some(Ending::Dropped) => return Self::Dropped,
            None => {}
        }
        if facts.status == Status::Live {
            return match &facts.asks {
                Some(question) => Self::Asks {
                    question: question.clone(),
                },
                None => Self::Working { turn: facts.turn },
            };
        }
        if let Some(files) = &facts.main_conflicts {
            return Self::ConflictsOnMain {
                files: files.clone(),
            };
        }
        match facts.status {
            Status::Interrupted => return Self::Interrupted,
            Status::Failed => return Self::Failed,
            Status::Live | Status::Stopped => {}
        }
        if let Some(queued) = facts.queued {
            return Self::Queued(queued);
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
    /// conflict, is ready to land, waits in the queue for the person to
    /// confirm its conflicts, or is a main chat with conflicts on it.
    pub fn needs_you(&self) -> bool {
        matches!(
            self,
            Self::Asks { .. }
                | Self::WouldConflict { .. }
                | Self::ReadyToLand { .. }
                | Self::ConflictsOnMain { .. }
                | Self::Queued(Queued {
                    needs_confirmation: true,
                    ..
                })
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
            Self::ConflictsOnMain { files } => {
                format!("Conflicts on main · {}", count(files.len(), "file"))
            }
            Self::Queued(Queued {
                needs_confirmation: true,
                ..
            }) => "Queued · needs confirmation".to_owned(),
            Self::Queued(_) => "Queued · lands after main's turn".to_owned(),
            Self::Interrupted => "Interrupted · tau closed".to_owned(),
            Self::Failed | Self::Landed | Self::Dropped | Self::Idle => {
                return None;
            }
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
            ending: run.ending.clone(),
            queued: self.queued(&run.id).map(|(position, waiting)| Queued {
                position,
                needs_confirmation: waiting.needs_confirmation(),
            }),
            main_conflicts: run
                .main_conflicts
                .as_ref()
                .map(|conflicts| conflicts.files.clone()),
        }
    }

    /// How many of `repo`'s open chats need the person
    /// ([`Attention::needs_you`]).
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
