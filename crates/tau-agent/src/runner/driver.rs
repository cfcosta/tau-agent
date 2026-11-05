//! What drives a run once the kernel assembled it (ADR 0045's
//! `tau.loop`): its turns, from the first request to its end, and an
//! idle run's compaction. The kernel opens the run, starts its plugins
//! and its session before, and keeps what came of it after; a [`Loop`]
//! is everything in between. [`Kernel`] is tau-agent's own, and what an
//! [`crate::agent::Agent`] drives with unless given another
//! ([`crate::agent::Agent::with_loop`]).

use async_trait::async_trait;
use tau_ai::message::UserContent;
use tau_store::StoreError;

use super::{RunResult, Runner, Turns};
use crate::{agent::Compacted, event::StopReason};

/// One run, assembled: its tools, plugins, session and store, ready to
/// take its first message. What a [`Loop`] drives.
pub struct LoopRun(pub(crate) Runner);

/// The run's steps, each keeping the runner's invariants whole: a
/// filler drives them in its own order and between them decides when
/// the run goes on.
impl LoopRun {
    /// Starts the run: `RunStart`, the first message stored, an inherited
    /// transcript offered for a rewrite.
    pub async fn begin(
        &mut self,
        input: UserContent,
    ) -> Result<Turns, StoreError> {
        self.0.begin(input).await
    }

    /// One turn, whole; the run's stop once it stops.
    pub async fn turn(
        &mut self,
        turns: &mut Turns,
    ) -> Result<Option<StopReason>, StoreError> {
        self.0.turn(turns).await
    }

    /// Ends the run with `stop`, storing its outcome.
    pub async fn end(
        self,
        turns: Turns,
        stop: StopReason,
    ) -> Result<RunResult, StoreError> {
        self.0.end(turns, stop).await
    }

    /// Offers the idle run's transcript for a rewrite, and stops.
    pub async fn compact_idle(self) -> Result<Option<Compacted>, StoreError> {
        self.0.compact_idle().await
    }
}

/// Drives runs. A filler must keep the runner's invariants: one stored
/// write per turn, every tool call answered, nothing asked after a
/// cancel, the run's events in their grammar. `tau_agent::testing`
/// checks a filler against them.
#[async_trait]
pub trait Loop: Send + Sync {
    /// Drives `run` from `input` to its end.
    async fn run(
        &self,
        run: LoopRun,
        input: UserContent,
    ) -> Result<RunResult, StoreError>;

    /// Offers an idle run's transcript for a rewrite, and stops.
    async fn compact_idle(
        &self,
        run: LoopRun,
    ) -> Result<Option<Compacted>, StoreError>;
}

/// tau-agent's own loop.
#[derive(Debug, Clone, Copy, Default)]
pub struct Kernel;

#[async_trait]
impl Loop for Kernel {
    async fn run(
        &self,
        run: LoopRun,
        input: UserContent,
    ) -> Result<RunResult, StoreError> {
        run.0.run(input).await
    }

    async fn compact_idle(
        &self,
        run: LoopRun,
    ) -> Result<Option<Compacted>, StoreError> {
        run.0.compact_idle().await
    }
}
