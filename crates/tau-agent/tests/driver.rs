//! What drives a run (ADR 0045's `tau.loop`): an agent given a loop of
//! its own drives every run with it, a resumed one too, and an idle
//! run's compaction; one given none drives them with tau-agent's own.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tau_agent::{
    agent::{Agent, Compacted},
    runner::{
        RunResult,
        driver::{IdleRun, Kernel, Loop, LoopRun},
    },
};
use tau_ai::message::UserContent;
use tau_store::StoreError;
use tau_testing::{block_on, scripted::ScriptedModel};
use tokio_util::sync::CancellationToken;

/// Takes part in an idle run's compaction, rewriting nothing.
struct Idle;

#[async_trait]
impl tau_agent::plugin::Plugin for Idle {
    fn name(&self) -> &str {
        "idle"
    }

    async fn start_idle(
        &self,
        _plan: &tau_agent::plugin::RunPlan,
        _ctx: &tau_agent::plugin::PluginCtx,
    ) -> Result<
        Option<Box<dyn tau_agent::plugin::PluginRun>>,
        tau_agent::error::PluginError,
    > {
        Ok(Some(Box::new(())))
    }
}

/// tau-agent's own loop, counting what it drives.
#[derive(Default)]
struct Counting {
    runs: AtomicUsize,
    idle: AtomicUsize,
}

#[async_trait]
impl Loop for Counting {
    async fn run(
        &self,
        run: LoopRun,
        input: UserContent,
    ) -> Result<RunResult, StoreError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Kernel.run(run, input).await
    }

    async fn compact_idle(
        &self,
        run: IdleRun,
    ) -> Result<Option<Compacted>, StoreError> {
        self.idle.fetch_add(1, Ordering::SeqCst);
        Kernel.compact_idle(run).await
    }
}

#[test]
fn an_agents_loop_drives_its_runs() {
    block_on(async {
        let model = ScriptedModel::new()
            .turn(|t| t.text("first"))
            .turn(|t| t.text("again"));
        let store = tau_store_sqlite::memory().await.unwrap();
        let counting = Arc::new(Counting::default());
        let agent = Agent::new(model.clone())
            .plugin(Idle)
            .with_loop(counting.clone());
        let mut run = agent.start("go", &store);
        let _: Vec<_> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.text, "first");
        assert_eq!(counting.runs.load(Ordering::SeqCst), 1);

        let mut resumed = agent.resume(&outcome.run).start("more", &store);
        let _: Vec<_> = resumed.events().collect().await;
        assert_eq!(resumed.outcome().await.unwrap().text, "again");
        assert_eq!(counting.runs.load(Ordering::SeqCst), 2);

        agent
            .resume(&outcome.run)
            .compact_idle(
                outcome.request,
                &store,
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(counting.idle.load(Ordering::SeqCst), 1);
        // An agent given no loop of its own drives with tau-agent's.
        assert_eq!(model.requests().len(), 2);
    });
}

/// A loop that ends the run as it begins, without asking the model: how
/// it ends it is the run's outcome.
struct Canned;

#[async_trait]
impl Loop for Canned {
    async fn run(
        &self,
        run: LoopRun,
        input: UserContent,
    ) -> Result<RunResult, StoreError> {
        let (run, turns) = run.begin(input).await?;
        run.end(turns, tau_agent::event::StopReason::Cancelled)
            .await
    }

    async fn compact_idle(
        &self,
        _run: IdleRun,
    ) -> Result<Option<Compacted>, StoreError> {
        Ok(None)
    }
}

#[test]
fn a_loop_in_place_of_the_kernels_decides_the_outcome() {
    block_on(async {
        let model = ScriptedModel::new().turn(|t| t.text("never asked"));
        let store = tau_store_sqlite::memory().await.unwrap();
        let agent = Agent::new(model.clone()).with_loop(Arc::new(Canned));
        let mut run = agent.start("go", &store);
        let id = run.id();
        let _: Vec<_> = run.events().collect().await;
        let outcome = run.outcome().await.unwrap();
        assert_eq!(outcome.stop, tau_agent::event::StopReason::Cancelled);
        assert!(model.requests().is_empty());
        // Its loop could only end it through the run's own end, so the
        // store has it ended too.
        let record = store.run(&id.0).await.unwrap().unwrap();
        assert_eq!(record.status, tau_store::Status::Cancelled);
    });
}
