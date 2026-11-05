//! What a loop ([`crate::runner::driver::Loop`]) is held to, for any
//! filler of `tau.loop` to check itself against (ADR 0045): the runner's
//! model, and the checks the tests that drive whole runs share.

pub mod loop_model;
pub mod support;

use async_trait::async_trait;
use tau_ai::message::UserContent;
use tau_store::StoreError;

use crate::{
    agent::Compacted,
    runner::{
        RunResult,
        driver::{Loop, LoopRun},
    },
};

/// Every loop tau-agent checks its run suites against: its own, and one
/// put together from the run's steps.
pub fn fillers() -> [std::sync::Arc<dyn Loop>; 2] {
    [
        std::sync::Arc::new(crate::runner::driver::Kernel),
        std::sync::Arc::new(Stepwise),
    ]
}

/// One of [`fillers`], drawn: what a property test drives its runs with,
/// so each case holds of any loop.
pub fn filler(tc: &hegel::TestCase) -> std::sync::Arc<dyn Loop> {
    use hegel::generators as gs;
    let [kernel, stepwise] = fillers();
    if tc.draw(gs::booleans()) {
        stepwise
    } else {
        kernel
    }
}

/// A loop put together from the run's steps ([`LoopRun::begin`],
/// [`LoopRun::turn`], [`LoopRun::end`]), apart from
/// [`crate::runner::driver::Kernel`]'s own: what the model checks of any
/// filler, it checks of this one too.
#[derive(Debug, Clone, Copy, Default)]
pub struct Stepwise;

#[async_trait]
impl Loop for Stepwise {
    async fn run(
        &self,
        mut run: LoopRun,
        input: UserContent,
    ) -> Result<RunResult, StoreError> {
        let mut turns = run.begin(input).await?;
        let stop = loop {
            match run.turn(&mut turns).await? {
                Some(stop) => break stop,
                None => continue,
            }
        };
        run.end(turns, stop).await
    }

    async fn compact_idle(
        &self,
        run: LoopRun,
    ) -> Result<Option<Compacted>, StoreError> {
        run.compact_idle().await
    }
}
