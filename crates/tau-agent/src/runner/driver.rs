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
/// take its first message. What a [`Loop`] drives. Its steps keep the
/// runner's invariants whole, and their types keep their order: it
/// begins once, its turns come after, and only its end gives the
/// [`RunResult`] a loop returns, so a run a loop drove is always ended in
/// the store.
pub struct LoopRun(pub(crate) Runner);

/// A run that began: its turns, then its end.
pub struct BegunRun(Runner);

/// An idle run, assembled to offer its transcript for a rewrite: no
/// start, no turn, no end.
pub struct IdleRun(pub(crate) Runner);

impl LoopRun {
    /// Starts the run: `RunStart`, the first message stored, an inherited
    /// transcript offered for a rewrite.
    pub async fn begin(
        mut self,
        input: UserContent,
    ) -> Result<(BegunRun, Turns), StoreError> {
        let turns = self.0.begin(input).await?;
        Ok((BegunRun(self.0), turns))
    }
}

impl BegunRun {
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
}

impl IdleRun {
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
        run: IdleRun,
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
        run: IdleRun,
    ) -> Result<Option<Compacted>, StoreError> {
        run.0.compact_idle().await
    }
}
