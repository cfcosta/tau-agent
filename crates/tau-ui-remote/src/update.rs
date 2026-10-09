//! What a host tells an interface, as one enum (decision 0013).
//!
//! A desktop host applies each [`HostUpdate`] to its own
//! [`Workspace`](crate::Workspace) with
//! [`Workspace::apply`](crate::Workspace::apply); a phone gets the same
//! updates over the wire and applies them the same way, so both
//! interfaces take one path.

use serde::{Deserialize, Serialize};
use tau_agent::{event::RunEvent, tool::RunId};
use tau_vcs::Landing;

use crate::{
    catalog::{Catalog, Repo},
    models::ModelChoice,
    pull_request::{PrState, PullRequest},
    push::{PushFailure, Pushed},
    setup::SetupUpdate,
    view::{CodeState, RunView},
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostUpdate {
    /// What a run streamed.
    Event(RunEvent),
    /// How long a run's model reasoned, from its turn asking it to its
    /// first answer (`answered`) or to the turn's end, by the clock of
    /// the computer that runs it: every interface shows these seconds,
    /// however late it hears of the turn. Comes before the event that
    /// stopped the clock.
    Reasoned {
        run: RunId,
        secs: u64,
        answered: bool,
    },
    /// Past runs, as the host loaded them.
    History(Vec<RunView>),
    /// A run the host just started, forked or resumed.
    Run(Box<RunView>),
    Catalog(Box<Catalog>),
    /// Something went wrong that the person should hear about.
    Alert {
        title: String,
        message: String,
    },
    /// A run the ChatGPT plan stopped, and what to do next.
    PlanRefusal(tau_ai::refusal::Refusal),
    QueryResult(Result<tau_store::Table, String>),
    PullRequest {
        run: RunId,
        pr: Box<PullRequest>,
    },
    PullRequestState {
        run: RunId,
        state: PrState,
    },
    /// What pushing a repository's trunk to GitHub did (ADR 0023).
    Pushed {
        repo: String,
        result: Result<Pushed, PushFailure>,
    },
    /// What landing a child run would do.
    LandingPreview {
        run: RunId,
        preview: Result<Landing, String>,
    },
    /// What landing a finished fork would do, worked out in the
    /// background; none when it cannot land now.
    Forecast {
        run: RunId,
        forecast: Option<crate::attention::Forecast>,
    },
    /// What landing a child run did.
    Landed {
        run: RunId,
        landing: Result<Landing, String>,
    },
    Dropped {
        run: RunId,
        result: Result<(), String>,
    },
    /// A main chat's landing queue and the conflicts on its stack, as
    /// the host has them now (ADR 0024): the chats waiting to land on
    /// it, in order, and what a turn left in conflict.
    LandingQueue {
        main: RunId,
        queue: Vec<crate::queue::Waiting>,
        conflicts: Option<crate::queue::MainConflicts>,
    },
    /// A landing tau closed in the middle of, which it finished as it
    /// started: its card goes in the parent's chat, and the child closes.
    LandingFinished(crate::view::LandingRecord),
    /// A chat landed by itself and went on (ADR 0034): its card goes in
    /// its parent's chat and in its own.
    LandedItself(crate::view::LandingRecord),
    /// tau started a run's next turn itself, with this message: resolving
    /// what a landing left in conflict (ADR 0014).
    TauTurn {
        run: RunId,
        prompt: String,
    },
    /// The code of a run and one of its forks, to compare.
    BranchCode {
        main: RunId,
        fork: RunId,
        code: CodeState,
    },
    /// The host took a message for a live run, which reads it as its
    /// next turn starts.
    Steered {
        run: RunId,
        text: String,
    },
    /// The host took a message to a finished run: it shows, the run
    /// moves to the top, opens again if it was closed, and starts again
    /// on `model`.
    Resumed {
        run: RunId,
        prompt: String,
        model: ModelChoice,
    },
    /// A run asked to go on could not.
    ResumeFailed(RunId),
    /// A body a plugin's host half stored with a run: the run's view
    /// folds it, as if the run had published it.
    PluginRecord {
        run: RunId,
        plugin: String,
        body: serde_json::Value,
    },
    /// A body to fold into a run's view without storing it: what a plugin
    /// says as the run starts or goes on.
    PluginFold {
        run: RunId,
        plugin: String,
        body: serde_json::Value,
    },
    /// A plugin's state in a run as `records` leave it, where what the
    /// interface showed could not be saved.
    PluginRestate {
        run: RunId,
        plugin: String,
        records: Vec<serde_json::Value>,
    },
    /// What a plugin's host half answered its UI.
    PluginReply {
        plugin: String,
        reply: serde_json::Value,
    },
    /// A model wrote a run's title.
    Titled {
        run: RunId,
        title: String,
    },
    /// A repository was added, with its main chat: the sidebar lists
    /// the chat under it, so it needs the chat's view as well as its id.
    Repo {
        repo: Repo,
        main: Option<Box<RunView>>,
    },
    /// Onboarding moved on. Only the machine being set up shows it.
    Setup(SetupUpdate),
    /// A fork's branch was kept, and its siblings dropped.
    BranchKept(RunId),
    /// A conversation closed: it leaves the sidebar, and History keeps
    /// it.
    Closed(RunId),
    /// Everything an interface shows alike, for one that just connected:
    /// what the host's interface shows.
    Snapshot(Box<crate::workspace::Synced>),
}

impl HostUpdate {
    pub fn alert(title: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Alert {
            title: title.into(),
            message: message.into(),
        }
    }

    pub fn catalog(catalog: Catalog) -> Self {
        Self::Catalog(Box::new(catalog))
    }
}
