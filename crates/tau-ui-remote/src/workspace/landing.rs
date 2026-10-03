//! Landing a fork or a sub-agent's work on its parent, or dropping it.

use super::*;

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
    /// for it, the child's chat closes, and the parent opens.
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
        self.landings.remove(run);
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
        self.close_run(run, cx);
        self.navigate(Route::Run(parent), cx);
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
        self.close_run(run, cx);
        if let Some(parent) = parent {
            self.navigate(Route::Run(parent), cx);
        }
    }

    pub fn landing(&self, run: &RunId) -> Option<&LandingState> {
        self.landings.get(run)
    }

    pub fn keep_branch(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.kept_branch = Some(run.clone());
        cx.emit(WorkspaceEvent::KeepBranch { run: run.clone() });
        cx.notify();
    }
}
