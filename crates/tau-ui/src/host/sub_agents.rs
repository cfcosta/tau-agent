//! Stopping a sub-agent's chat on its own (ADR 0009): a chat of its own,
//! it has a Stop like any other. A sub-agent runs inside its main chat's
//! `delegate` call, which the host did not start, so the host learns its
//! stop as it starts.

use tau_agent::plugin::FinishedRun;
use tokio_util::sync::CancellationToken;

use super::*;

/// Each sub-agent's stop while it runs, by run. Stopping one fails only
/// its `delegate` call: its main chat goes on, and nothing of it lands.
#[derive(Clone, Default)]
pub(crate) struct SubAgentStops(Arc<Mutex<HashMap<RunId, CancellationToken>>>);

impl SubAgentStops {
    /// Stops `run` if it is a sub-agent going on. Returns whether it was.
    pub(crate) fn cancel(&self, run: &RunId) -> bool {
        let stops = self.0.lock().expect("not poisoned");
        let Some(stop) = stops.get(run) else {
            return false;
        };
        stop.cancel();
        true
    }
}

#[async_trait]
impl Plugin for SubAgentStops {
    fn name(&self) -> &str {
        "tau-ui-sub-agents"
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        self.0
            .lock()
            .expect("not poisoned")
            .insert(ctx.run.clone(), ctx.cancel.clone());
        Ok(Box::new(Forget(self.clone())))
    }
}

/// Forgets the sub-agent's stop once it ended.
struct Forget(SubAgentStops);

#[async_trait]
impl PluginRun for Forget {
    async fn finish(&mut self, _run: &FinishedRun<'_>, ctx: &PluginCtx) {
        self.0.0.lock().expect("not poisoned").remove(&ctx.run);
    }
}
