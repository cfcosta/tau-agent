//! What a host tells an interface, as one enum (decision 0013).
//!
//! A desktop host applies each [`HostUpdate`] to its own
//! [`Workspace`](crate::Workspace) with
//! [`Workspace::apply`](crate::Workspace::apply); a phone gets the same
//! updates over the wire and applies them the same way, so both
//! interfaces take one path.

use serde::{Deserialize, Serialize};
use tau_agent::{event::RunEvent, tool::RunId};
use tau_ui_plugin::vcs::Landing;

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
    /// A run the provider stopped, and what to do next, in its words.
    Stopped(tau_ui_plugin::provider::Stopped),
    QueryResult(Result<tau_store::Table, String>),
    PullRequest {
        run: RunId,
        pr: Box<PullRequest>,
    },
    PullRequestState {
        run: RunId,
        state: PrState,
    },
    /// What pushing a repository's trunk to the forge did (ADR 0023).
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
        /// The images sent with it.
        #[serde(default)]
        pictures: Vec<crate::view::Picture>,
    },
    /// The images sent with the message `run` just read, as it was
    /// steered with it: they show under it.
    Pictures {
        run: RunId,
        pictures: Vec<crate::view::Picture>,
    },
    /// What the host has of a picture an interface asked for
    /// (`WorkspaceEvent::Picture`): its bytes in base64, or why it has
    /// none. Phones get it unnumbered, and not again once back.
    Picture {
        picture: crate::view::Picture,
        data: Option<String>,
        error: Option<String>,
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
        /// The phone's call whose action it answers, when a phone's
        /// click asked (ADR 0046).
        #[serde(default)]
        asker: Option<u64>,
    },
    /// What a phone's call to a plugin asks of its own window, after the
    /// computer carried it out (ADR 0046).
    Called {
        id: u64,
        plugin: String,
        requests: Vec<tau_ui_plugin::Request>,
    },
    /// Plugins' views the computer built for phones, each for the key a
    /// phone asked by (ADR 0046).
    Views {
        answers:
            Vec<(tau_ui_plugin::wire::ViewKey, tau_ui_plugin::wire::Built)>,
    },
    /// The picture `key` a plugin's host half answered a tree's image
    /// with (ADR 0045).
    PluginFetched {
        plugin: String,
        key: String,
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
    /// A change to a plugin's UI state, from any interface (ADR 0045).
    Ui(tau_ui_plugin::UiEvent),
    /// Every plugin's UI state, as the host kept it.
    UiState(tau_ui_plugin::UiState),
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

impl HostUpdate {
    /// This update as phones are sent it: without what plugins that only
    /// the computer may reach keep (`Declaration::computer_only`), such as
    /// a sign-in or the pairing code, in the catalog or in their views.
    pub fn for_phones(mut self) -> Self {
        let kept = |plugin: &str| !computer_only(plugin);
        match &mut self {
            Self::Catalog(catalog) => catalog.strip_for_phones(),
            Self::Snapshot(synced) => synced.catalog.strip_for_phones(),
            Self::Views { answers } => {
                for (key, built) in answers.iter_mut() {
                    if key.view.plugin().is_some_and(|plugin| !kept(plugin)) {
                        *built = tau_ui_plugin::wire::Built::Nothing;
                    }
                    if let tau_ui_plugin::wire::Built::Outs { outs } = built {
                        outs.retain(|out| kept(&out.plugin));
                    }
                    if let tau_ui_plugin::wire::Built::Commands { commands } =
                        built
                    {
                        commands.retain(|(plugin, _)| kept(plugin));
                    }
                }
            }
            _ => {}
        }
        self
    }
}

/// Whether only the computer may reach `plugin` (`Declaration::computer_only`).
pub fn computer_only(plugin: &str) -> bool {
    crate::plugins::registry()
        .get(plugin)
        .is_some_and(|erased| erased.declaration().reach.computer_only)
}

impl Catalog {
    /// Takes away what plugins only the computer may reach keep: what
    /// phones are sent of the catalog ([`HostUpdate::for_phones`]).
    pub fn strip_for_phones(&mut self) {
        let kept = |plugin: &String, _: &mut _| !computer_only(plugin);
        self.plugin_data.retain(kept);
        self.plugin_settings.retain(kept);
        for settings in self.repo_plugin_settings.values_mut() {
            settings.retain(kept);
        }
        for repo in &mut self.repos {
            repo.plugins.retain(kept);
        }
    }

    /// The catalog as phones are sent it.
    pub fn for_phones(mut self) -> Self {
        self.strip_for_phones();
        self
    }
}
