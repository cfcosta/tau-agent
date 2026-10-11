//! What a plugin's host half drives of this host's conversations (ADR
//! 0037): an agent's repository, made and listed, and turns of its main
//! chat that the agent's events start.
//!
//! Whether a main chat is going is read here. Making an agent and
//! starting a turn go to the interface, which shows the turn as it
//! starts, then has the host start it ([`Host::attach`] answers them).

use tau_ui_plugin::{Conversations, Turn, registry::HostFuture};
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
}
