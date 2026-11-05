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
