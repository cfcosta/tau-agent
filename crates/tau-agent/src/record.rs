//! A run's record in the store, from the run's side: `running` while it
//! goes on, then ended once, with how it ended. However the run stops,
//! its ending reaches the store: by [`crate::runner::Runner::end`], or,
//! when the run fails on the way, as failed. An ending already stored is
//! never written over.

use std::sync::Arc;

use statig::{
    Outcome::{self, Handled, Transition},
    blocking::{IntoStateMachineExt as _, StateMachine},
    state_machine,
};
use tau_store::{Status, Store, StoreError};
use tokio::sync::Mutex;

use crate::{agent::AgentError, tool::RunId};

/// How a run ended, as the store keeps it.
#[derive(Clone)]
pub(crate) struct Ending {
    pub(crate) status: Status,
    pub(crate) result: Option<String>,
    pub(crate) error: Option<String>,
}

/// The record of one run: shared by the run's task and its runner,
/// which the loop owns.
#[derive(Clone)]
pub(crate) struct RunRecord {
    store: Store,
    run: RunId,
    record: Arc<Mutex<StateMachine<Record>>>,
}

/// What happens to a record.
pub(crate) enum Input {
    /// The run ended so.
    End(Ending),
    /// The ending the record asked for is stored.
    Stored,
}

#[derive(Default)]
pub(crate) struct Record;

impl RunRecord {
    /// The record of `run`, `running` in `store`.
    pub(crate) fn running(store: Store, run: RunId) -> Self {
        Self {
            store,
            run,
            record: Arc::new(Mutex::new(Record.state_machine())),
        }
    }

    /// Stores how the run ended, unless an ending is stored already.
    pub(crate) async fn end(&self, ending: Ending) -> Result<(), StoreError> {
        let mut record = self.record.lock().await;
        let mut write = None;
        record.handle_with_context(&Input::End(ending), &mut write);
        let Some(ending) = write else {
            return Ok(());
        };
        self.store
            .finish_run(
                &self.run.0,
                ending.status,
                ending.result.as_deref(),
                ending.error.as_deref(),
            )
            .await?;
        record.handle_with_context(&Input::Stored, &mut None);
        Ok(())
    }

    /// Ends the run as failed with `error`, unless it ended already, and
    /// gives the error back. If even that write fails, the store's error
    /// wins: the run is left `running`, for the next start of the app.
    pub(crate) async fn fail(&self, error: AgentError) -> AgentError {
        let ending = Ending {
            status: Status::Failed,
            result: None,
            error: Some(error.to_string()),
        };
        match self.end(ending).await {
            Ok(()) => error,
            Err(store_error) => AgentError::Store(store_error),
        }
    }
}

#[state_machine(initial = "State::running()")]
impl Record {
    /// Going on, or its ending not stored yet: an ending is to be
    /// written, in `context`.
    #[state]
    fn running(context: &mut Option<Ending>, event: &Input) -> Outcome<State> {
        match event {
            Input::End(ending) => {
                *context = Some(ending.clone());
                Handled
            }
            Input::Stored => Transition(State::ended()),
        }
    }

    /// Its ending is stored: nothing is written again.
    #[state]
    fn ended() -> Outcome<State> {
        Handled
    }
}
