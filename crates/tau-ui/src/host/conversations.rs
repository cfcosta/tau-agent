//! What a plugin's host half drives of this host's conversations (ADR
//! 0037): an agent's repository, made and listed, turns of its main
//! chat that the agent's events start, and any chat a plugin wakes
//! later (ADR 0039).
//!
//! Whether a main chat is going is read here. Making an agent and
//! starting a turn go to the interface, which shows the turn as it
//! starts, then has the host start it ([`Host::attach`] answers them).

use tau_ui_plugin::{Conversations, Turn, Waking, registry::HostFuture};
use tokio::sync::oneshot;

use super::*;

/// What a host half asked of the host, with where its answer goes.
pub(super) enum Ask {
    ListAgent {
        name: String,
        done: oneshot::Sender<anyhow::Result<()>>,
    },
    Turn {
        turn: Turn,
        done: oneshot::Sender<anyhow::Result<()>>,
    },
    Wake {
        run: RunId,
        text: String,
        done: oneshot::Sender<anyhow::Result<Waking>>,
    },
}

/// [`Conversations`] on this host.
pub(super) struct HostConversations {
    pub(super) runs: Arc<Mutex<HashMap<RunId, RunControl>>>,
    pub(super) list: Arc<Mutex<RepoList>>,
    pub(super) asks: mpsc::UnboundedSender<Ask>,
}

impl HostConversations {
    /// Sends what `ask` makes of `done` to the interface, and waits for
    /// its answer.
    fn ask(
        &self,
        ask: impl FnOnce(oneshot::Sender<anyhow::Result<()>>) -> Ask,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        let (done, answer) = oneshot::channel();
        let sent = self.asks.send(ask(done));
        Box::pin(async move {
            sent.map_err(|_| anyhow::anyhow!("tau is closing"))?;
            answer.await.map_err(|_| {
                anyhow::anyhow!("tau's window closed before it answered")
            })?
        })
    }
}

impl Conversations for HostConversations {
    fn list_agent(&self, name: &str) -> HostFuture<'_, anyhow::Result<()>> {
        let name = name.to_owned();
        self.ask(move |done| Ask::ListAgent { name, done })
    }

    fn main_of(&self, repo: &str) -> Option<String> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .find(|listed| listed.name == repo)
            .and_then(|listed| listed.main.clone())
    }

    fn running(&self, repo: &str) -> bool {
        self.main_of(repo).is_some_and(|main| {
            self.runs
                .lock()
                .expect("not poisoned")
                .contains_key(&RunId(main.into()))
        })
    }

    fn turn(&self, turn: Turn) -> HostFuture<'_, anyhow::Result<()>> {
        self.ask(move |done| Ask::Turn { turn, done })
    }

    /// A chat with a turn going is busy here; the interface starts the
    /// others, as it does an agent's turns.
    fn wake(
        &self,
        run: &str,
        text: String,
    ) -> HostFuture<'_, anyhow::Result<Waking>> {
        let run = RunId(run.into());
        if self.runs.lock().expect("not poisoned").contains_key(&run) {
            return Box::pin(async { Ok(Waking::Busy) });
        }
        let (done, answer) = oneshot::channel();
        let sent = self.asks.send(Ask::Wake { run, text, done });
        Box::pin(async move {
            sent.map_err(|_| anyhow::anyhow!("tau is closing"))?;
            answer.await.map_err(|_| {
                anyhow::anyhow!("tau's window closed before it answered")
            })?
        })
    }
}
