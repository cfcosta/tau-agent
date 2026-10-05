//! How long a run's model reasons: from the turn asking it to its first
//! answer, text or a call. The transcript counts the seconds while it
//! waits, and keeps them on the reasoning once it answers.

use super::*;

impl Workspace {
    /// Starts a run's clock when its turn asks the model, and stops it
    /// when the model answers or the turn ends, before `event` applies.
    /// The seconds go on the reasoning the model streamed; a model that
    /// streamed none gets a reasoning item of its own, holding only
    /// them, once it answers.
    pub(super) fn time_reasoning(
        &mut self,
        event: &RunEvent,
        cx: &mut Context<Self>,
    ) {
        let (run, answers) = match event {
            RunEvent::TurnStart { run, .. } => {
                self.asked.insert(run.clone(), Instant::now());
                self.tick(cx);
                return;
            }
            RunEvent::TextDelta { run, .. }
            | RunEvent::ToolCallDelta { run, .. }
            | RunEvent::ToolStart {
                run, parent: None, ..
            } => (run, true),
            RunEvent::TurnEnd { run, .. } | RunEvent::RunEnd { run, .. } => {
                (run, false)
            }
            _ => return,
        };
        let Some(since) = self.asked.remove(run) else {
            return;
        };
        let secs = since.elapsed().as_secs();
        let Some(view) = self.runs.iter_mut().find(|view| view.id == *run)
        else {
            return;
        };
        match view.items.last_mut() {
            Some(Item::Thinking {
                secs: open @ None, ..
            }) => {
                *open = Some(secs);
            }
            _ if answers && secs > 0 => view.items.push(Item::Thinking {
                text: String::new(),
                secs: Some(secs),
            }),
            _ => {}
        }
    }

    /// How long `run` has waited on its model, while it does.
    pub fn reasoning_for(&self, run: &RunId) -> Option<Duration> {
        self.asked.get(run).map(Instant::elapsed)
    }

    /// Redraws once a second while any run waits on its model.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let waiting = this.update(cx, |ws, cx| {
                    ws.ticking = !ws.asked.is_empty();
                    if ws.ticking {
                        cx.notify();
                    }
                    ws.ticking
                });
                if !matches!(waiting, Ok(true)) {
                    return;
                }
            }
        })
        .detach();
    }
}
