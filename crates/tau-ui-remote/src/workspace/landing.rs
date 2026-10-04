//! Landing a fork or a sub-agent's work on its parent, or dropping it.

use super::*;
use crate::queue::{MainConflicts, Waiting};

impl Workspace {
    /// Asks what landing `run` on its parent would do (ADR 0014).
    pub fn preview_landing(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::Previewing);
        cx.emit(WorkspaceEvent::PreviewLanding { run: run.clone() });
        cx.notify();
    }

    /// tau starts `run`'s next turn itself with `prompt`: resolving what a
    /// landing left in conflict (ADR 0014). It shows as tau's.
    pub fn tau_turn(
        &mut self,
        run: &RunId,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.runs.iter().position(|view| &view.id == run) else {
            return;
        };
        let mut view = self.runs.remove(at);
        self.closed.remove(run);
        self.resuming.insert(run.clone(), view.status.clone());
        view.items.push(Item::Tau(prompt));
        view.status = RunStatus::Planning;
        self.runs.insert(0, view);
        cx.notify();
    }

    pub fn set_landing_preview(
        &mut self,
        run: &RunId,
        preview: Result<Landing, String>,
        cx: &mut Context<Self>,
    ) {
        self.landings
            .insert(run.clone(), LandingState::Preview(preview));
        cx.notify();
    }

    /// Puts a landing preview away without landing.
    pub fn cancel_landing(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.remove(run);
        cx.notify();
    }

    /// Lands `run` on its parent.
    pub fn land(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::Landing);
        cx.emit(WorkspaceEvent::Land { run: run.clone() });
        cx.notify();
    }

    /// What landing `run` came to. Landed, the parent's chat gets a card
    /// for it, the child's chat closes, and the parent opens when the
    /// person was waiting on this landing.
    pub fn landed(
        &mut self,
        run: &RunId,
        landing: Result<Landing, String>,
        cx: &mut Context<Self>,
    ) {
        let landing = match landing {
            Ok(landing) => landing,
            Err(error) => {
                self.landings
                    .insert(run.clone(), LandingState::Preview(Err(error)));
                cx.notify();
                return;
            }
        };
        // The person who just asked for it follows it to its parent; a
        // chat that waited in the queue lands while they are elsewhere.
        let asked =
            matches!(self.landings.remove(run), Some(LandingState::Landing));
        let Some(child) = self.run(run) else {
            return;
        };
        let parent = match &child.origin {
            Origin::Fork { from, .. } => from.clone(),
            Origin::SubAgent { parent } => parent.clone(),
            Origin::Root => return,
        };
        let card = LandedCard::from_record(LandingRecord {
            from: run.0.to_string(),
            title: child.title.clone(),
            landing,
            recovered: false,
        });
        let changes = card.changes.len();
        if let Some(view) = self.runs.iter_mut().find(|view| view.id == parent)
        {
            view.items.retain(
                |item| !matches!(item, Item::ForkReady { fork } if fork == run),
            );
            view.items.push(Item::Landed(card));
        }
        if let Some(view) = self.runs.iter_mut().find(|view| &view.id == run) {
            view.ending = Some(Ending::Landed {
                on: parent.clone(),
                changes,
            });
        }
        // Every interface applies this alike; the host keeps it closed.
        self.closed.insert(run.clone());
        cx.emit(WorkspaceEvent::CloseRun { run: run.clone() });
        if asked {
            self.navigate(Route::Run(parent), cx);
        }
    }

    /// A landing tau finished as it started, after closing in its
    /// middle: the parent's chat gets its card, unless history brought
    /// it already, and the child is closed, landed. Nothing opens.
    pub fn landing_finished(
        &mut self,
        record: LandingRecord,
        cx: &mut Context<Self>,
    ) {
        let run = RunId(record.from.as_str().into());
        let Some(parent) = self
            .run(&run)
            .and_then(|child| child.origin.parent().cloned())
        else {
            return;
        };
        let changes = record.landing.changes.len();
        if let Some(view) = self.runs.iter_mut().find(|view| view.id == parent)
        {
            view.items.retain(
                |item| !matches!(item, Item::ForkReady { fork } if *fork == run),
            );
            let shown = view.items.iter().any(
                |item| matches!(item, Item::Landed(card) if card.from == run),
            );
            if !shown {
                view.items
                    .push(Item::Landed(LandedCard::from_record(record)));
            }
        }
        if let Some(view) = self.runs.iter_mut().find(|view| view.id == run) {
            view.ending = Some(Ending::Landed {
                on: parent,
                changes,
            });
        }
        self.closed.insert(run.clone());
        cx.emit(WorkspaceEvent::CloseRun { run });
        cx.notify();
    }

    /// Asks before dropping `run`, a child run.
    pub fn ask_drop(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::ConfirmDrop);
        cx.notify();
    }

    /// Drops `run`: its own changes are abandoned, and it closes.
    pub fn drop_child(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::Dropping);
        cx.emit(WorkspaceEvent::DropChild { run: run.clone() });
        cx.notify();
    }

    /// What dropping `run` came to. Dropped, its chat closes and its
    /// parent opens.
    pub fn dropped(
        &mut self,
        run: &RunId,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = result {
            self.landings
                .insert(run.clone(), LandingState::Preview(Err(error)));
            cx.notify();
            return;
        }
        self.landings.remove(run);
        let parent = self.run(run).and_then(|child| match &child.origin {
            Origin::Fork { from, .. } => Some(from.clone()),
            Origin::SubAgent { parent } => Some(parent.clone()),
            Origin::Root => None,
        });
        if let Some(view) = parent.as_ref().and_then(|parent| {
            self.runs.iter_mut().find(|view| &view.id == parent)
        }) {
            view.items.retain(
                |item| !matches!(item, Item::ForkReady { fork } if fork == run),
            );
        }
        if let Some(view) = self.runs.iter_mut().find(|view| &view.id == run) {
            view.ending = Some(Ending::Dropped);
        }
        // Every interface applies this alike; the host keeps it closed.
        self.closed.insert(run.clone());
        cx.emit(WorkspaceEvent::CloseRun { run: run.clone() });
        if let Some(parent) = parent {
            self.navigate(Route::Run(parent), cx);
        }
    }

    /// Goes on with `run`, which tau closing cut off: the host shows
    /// tau's message saying so, and resumes it in its workspace.
    pub fn resume_cut_off(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.follow = true;
        cx.emit(WorkspaceEvent::ResumeCutOff { run: run.clone() });
        cx.notify();
    }

    /// What the host says of `main`'s landing queue and the conflicts
    /// on its stack. A chat that joined the queue is no longer landing:
    /// its card says where it waits.
    pub fn set_landing_queue(
        &mut self,
        main: &RunId,
        queue: Vec<Waiting>,
        conflicts: Option<MainConflicts>,
        cx: &mut Context<Self>,
    ) {
        for waiting in &queue {
            self.landings.remove(&RunId(waiting.run.as_str().into()));
        }
        if let Some(view) = self.runs.iter_mut().find(|view| &view.id == main) {
            view.landing_queue = queue;
            view.main_conflicts = conflicts;
        }
        cx.notify();
    }

    /// Where `run` waits to land, if it does: its place in its main
    /// chat's queue, from 1, and what the host last found of it.
    pub fn queued(&self, run: &RunId) -> Option<(usize, &Waiting)> {
        let parent = self.run(run)?.origin.parent()?;
        self.run(parent)?
            .landing_queue
            .iter()
            .enumerate()
            .find(|(_, waiting)| *waiting.run == *run.0)
            .map(|(at, waiting)| (at + 1, waiting))
    }

    /// Takes `run` out of its main chat's landing queue.
    pub fn unqueue(&mut self, run: &RunId, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::Unqueue { run: run.clone() });
        cx.notify();
    }

    /// Starts tau's resolving turn on `main` again.
    pub fn resolve_again(&mut self, main: &RunId, cx: &mut Context<Self>) {
        self.follow = true;
        cx.emit(WorkspaceEvent::ResolveAgain { main: main.clone() });
        cx.notify();
    }

    /// The person will resolve `main`'s conflicts themselves: the card
    /// goes; main stays marked until it is clean.
    pub fn dismiss_conflicts(&mut self, main: &RunId, cx: &mut Context<Self>) {
        if let Some(conflicts) = self
            .runs
            .iter_mut()
            .find(|view| &view.id == main)
            .and_then(|view| view.main_conflicts.as_mut())
        {
            conflicts.dismissed = true;
        }
        cx.emit(WorkspaceEvent::DismissConflicts { main: main.clone() });
        cx.notify();
    }

    pub fn landing(&self, run: &RunId) -> Option<&LandingState> {
        self.landings.get(run)
    }

    pub fn keep_branch(&mut self, run: &RunId, cx: &mut Context<Self>) {
        // It shows once the host keeps it (`HostUpdate::BranchKept`).
        cx.emit(WorkspaceEvent::KeepBranch { run: run.clone() });
        cx.notify();
    }
}
