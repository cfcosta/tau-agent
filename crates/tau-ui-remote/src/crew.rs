//! A main chat's crew (ADR 0031): the sub-agents it spawned, as its
//! chat's tray shows them. Sub-agents are disposable: they live in the
//! tray while they work and for the round that spawned them, not in the
//! sidebar, which keeps the forks the person made. History keeps them
//! all.

use std::collections::HashSet;

use tau_agent::{event::StopReason, tool::RunId};

use crate::{
    view::{Ending, Item, Origin, RunStatus, RunView},
    workspace::Workspace,
};

/// Where a sub-agent in the tray stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Standing {
    /// It works: on its `turn`, its latest tool call `doing`, at `cost`
    /// so far.
    Working {
        turn: u32,
        doing: Option<String>,
        cost: f64,
    },
    /// It finished and waits in main's queue.
    WaitingToLand,
    /// Its `changes` landed on main.
    Landed { changes: usize },
    /// It failed, or tau closed while it ran: its changes were dropped.
    Failed,
    /// The person stopped it: its changes were dropped.
    Stopped,
}

impl Standing {
    pub fn is_working(&self) -> bool {
        matches!(self, Self::Working { .. })
    }
}

/// One sub-agent in the tray.
pub struct Member<'a> {
    pub run: &'a RunView,
    pub standing: Standing,
}

impl Workspace {
    /// The sub-agents `main` spawned that the tray shows, newest first:
    /// every one still working, and those of the current round, which
    /// starts with the person's last message to main. Report turns tau
    /// starts do not start a round.
    pub fn crew<'a>(&'a self, main: &'a RunView) -> Vec<Member<'a>> {
        let since = main
            .items
            .iter()
            .rposition(|item| matches!(item, Item::User(_)))
            .map_or(0, |at| at + 1);
        let round: HashSet<RunId> = main.items[since..]
            .iter()
            .filter_map(|item| match item {
                Item::Tool(card) if card.tool == tau_vcs::details::SPAWN => {
                    tau_vcs::ui::spawned(&card.data)
                }
                _ => None,
            })
            .collect();
        self.sub_agents_of(&main.id)
            .filter(|run| run.status.is_live() || round.contains(&run.id))
            .map(|run| Member {
                run,
                standing: standing(run),
            })
            .collect()
    }

    /// How many of `main`'s sub-agents are working.
    pub fn working_crew(&self, main: &RunId) -> usize {
        self.sub_agents_of(main)
            .filter(|run| run.status.is_live())
            .count()
    }

    /// `run`'s sub-agents with a view here, newest first.
    pub fn sub_agents_of<'a>(
        &'a self,
        run: &'a RunId,
    ) -> impl Iterator<Item = &'a RunView> {
        self.runs.iter().filter(move |view| {
            matches!(&view.origin, Origin::SubAgent { parent } if parent == run)
        })
    }

    /// The run the sidebar marks for `run`: a sub-agent's main chat,
    /// since sub-agents are not listed there, else `run` itself.
    pub fn listed_as<'a>(&self, run: &'a RunView) -> &'a RunId {
        match &run.origin {
            Origin::SubAgent { parent } => parent,
            _ => &run.id,
        }
    }
}

/// Where `run`, a sub-agent, stands.
pub fn standing(run: &RunView) -> Standing {
    match (&run.ending, &run.status) {
        (Some(Ending::Landed { changes, .. }), _) => {
            Standing::Landed { changes: *changes }
        }
        (Some(Ending::Dropped), RunStatus::Finished(StopReason::Cancelled)) => {
            Standing::Stopped
        }
        (Some(Ending::Dropped), _) => Standing::Failed,
        (None, status) if status.is_live() => Standing::Working {
            turn: run.turn,
            doing: run.items.iter().rev().find_map(|item| match item {
                Item::Tool(card) if card.summary.is_empty() => {
                    Some(card.tool.clone())
                }
                Item::Tool(card) => {
                    Some(format!("{} {}", card.tool, card.summary))
                }
                _ => None,
            }),
            cost: run.usage.cost,
        },
        (
            None,
            RunStatus::Finished(StopReason::Stop | StopReason::Limit(_)),
        ) => Standing::WaitingToLand,
        (None, RunStatus::Finished(StopReason::Cancelled)) => Standing::Stopped,
        (None, _) => Standing::Failed,
    }
}
