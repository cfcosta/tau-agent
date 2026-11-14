//! The `heiko.loop` slot (ADR 0045): what drives a run once the kernel
//! assembled it ([`heiko_agent::runner::driver::Loop`]). Nothing fills it
//! by default, and then heiko-agent's own loop drives
//! ([`heiko_agent::runner::driver::Kernel`]). A filler is a Rust plugin's:
//! it checks itself against the runner's model
//! (`heiko_agent::testing::loop_model`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use heiko_agent::{
    agent::Compacted,
    runner::{
        RunResult,
        driver::{IdleRun, Kernel, Loop, LoopRun},
    },
};
use heiko_ai::message::UserContent;
use heiko_store::StoreError;

use crate::slots::{Fills, Slot};

/// The slot.
pub const LOOP: Slot<dyn Loop> = Slot::new("heiko.loop");

/// A loop over whatever fills `heiko.loop` as each run starts, and
/// heiko-agent's own when nothing does: the one the host builds its agent
/// with. A run keeps the filler it started with.
#[derive(Clone)]
pub struct SlotLoop {
    slots: Arc<Mutex<Fills>>,
}

impl SlotLoop {
    pub fn new(slots: Arc<Mutex<Fills>>) -> Self {
        Self { slots }
    }

    /// What drives a run starting now.
    pub fn now(&self) -> Arc<dyn Loop> {
        self.slots
            .lock()
            .expect("not poisoned")
            .get(&LOOP)
            .unwrap_or_else(|| Arc::new(Kernel))
    }
}

#[async_trait]
impl Loop for SlotLoop {
    async fn run(
        &self,
        run: LoopRun,
        input: UserContent,
    ) -> Result<RunResult, StoreError> {
        self.now().run(run, input).await
    }

    async fn compact_idle(
        &self,
        run: IdleRun,
    ) -> Result<Option<Compacted>, StoreError> {
        self.now().compact_idle(run).await
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::StreamExt;
    use heiko_agent::agent::Agent;
    use heiko_testing::scripted::ScriptedModel;

    use super::*;

    /// heiko-agent's own loop, counting the runs it drives.
    #[derive(Default)]
    struct Counting(AtomicUsize);

    #[async_trait]
    impl Loop for Counting {
        async fn run(
            &self,
            run: LoopRun,
            input: UserContent,
        ) -> Result<RunResult, StoreError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Kernel.run(run, input).await
        }

        async fn compact_idle(
            &self,
            run: IdleRun,
        ) -> Result<Option<Compacted>, StoreError> {
            Kernel.compact_idle(run).await
        }
    }

    /// With nothing in the slot, heiko-agent's own loop drives; once a
    /// plugin fills it, the runs that start from then on are its.
    #[test]
    fn the_slots_filler_drives_the_runs_that_start_after_it() {
        heiko_testing::block_on(async {
            let slots = Arc::new(Mutex::new(Fills::default()));
            let model = ScriptedModel::new()
                .turn(|t| t.text("kernel"))
                .turn(|t| t.text("filler"));
            let agent = Agent::new(model)
                .with_loop(Arc::new(SlotLoop::new(slots.clone())));
            let store = heiko_store_sqlite::memory().await.unwrap();
            let mut run = agent.start("go", &store);
            let _: Vec<_> = run.events().collect().await;
            assert_eq!(run.outcome().await.unwrap().text, "kernel");

            let counting = Arc::new(Counting::default());
            {
                let mut fills = slots.lock().unwrap();
                fills.filling("looper", &[LOOP.name().to_owned()]);
                fills.fill(&LOOP, counting.clone() as Arc<dyn Loop>);
            }
            let mut run = agent.start("again", &store);
            let _: Vec<_> = run.events().collect().await;
            assert_eq!(run.outcome().await.unwrap().text, "filler");
            assert_eq!(counting.0.load(Ordering::SeqCst), 1);
        });
    }
}
