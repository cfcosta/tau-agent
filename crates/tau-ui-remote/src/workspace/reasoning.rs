//! How long a run's model reasons: from the turn asking it to its first
//! answer, text or a call. The transcript counts the seconds while it
//! waits, by each device's own clock. The seconds it keeps on the
//! reasoning are the computer's: the computer that runs the model times
//! it and tells every interface ([`HostUpdate::Reasoned`]), so a phone
//! that hears of a turn late shows what the computer shows.

use super::*;

impl Workspace {
    /// Applies what a run streamed, as the computer that runs it does:
    /// when `event` stops the run's reasoning clock, the seconds go to
    /// every interface first ([`HostUpdate::Reasoned`]), then the event.
    pub fn apply_streamed(&mut self, event: RunEvent, cx: &mut Context<Self>) {
        if let Some(reasoned) = self.stop_reasoning(&event) {
            self.apply(reasoned, cx);
        }
        self.apply(HostUpdate::Event(event), cx);
    }

    /// Starts a run's clock when its turn asks the model, and stops it
    /// when the model answers or the turn ends: the transcript counts
    /// while it runs.
    pub(super) fn watch_reasoning(
        &mut self,
        event: &RunEvent,
        cx: &mut Context<Self>,
    ) {
        if let RunEvent::TurnStart { run, .. } = event {
            self.asked.insert(run.clone(), Instant::now());
            self.tick(cx);
        } else {
            self.stop_reasoning(event);
        }
    }

    /// Stops the run's clock if `event` answers or ends its turn, with
    /// how long it ran.
    fn stop_reasoning(&mut self, event: &RunEvent) -> Option<HostUpdate> {
        let (run, answered) = match event {
            RunEvent::TextDelta { run, .. }
            | RunEvent::ToolCallDelta { run, .. }
            | RunEvent::ToolStart {
                run, parent: None, ..
            } => (run, true),
            RunEvent::TurnEnd { run, .. } | RunEvent::RunEnd { run, .. } => {
                (run, false)
            }
            _ => return None,
        };
        let since = self.asked.remove(run)?;
        Some(HostUpdate::Reasoned {
            run: run.clone(),
            secs: since.elapsed().as_secs(),
            answered,
        })
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
