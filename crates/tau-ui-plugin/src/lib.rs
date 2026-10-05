//! The interface a tau plugin brings its UI through (ADR 0017).
//!
//! A plugin is a [`UiPlugin`]: the agent plugin it builds for a run, the
//! fold that turns what it publishes into state, and the UI it adds to
//! the interface. There is no plugin without a UI: `tau-ui` builds a
//! run's plugins only through its [`Registry`].
//!
//! - **On the host**, a plugin keeps its own state ([`UiPlugin::Host`]),
//!   builds its agent plugin for each run ([`UiPlugin::agent_plugins`]),
//!   describes itself ([`UiPlugin::catalog`]), sends the interface its
//!   data ([`UiPlugin::data`], [`UiPlugin::repo_data`]), and carries out
//!   what its UI asks ([`UiPlugin::act`]).
//! - **Wherever a run is shown**, live or stored, on a computer or a
//!   phone, the plugin's state folds each record it published
//!   ([`Fold::apply`]), and places anchors in the transcript and on tool
//!   cards ([`RunCx`]).
//! - **In the interface**, [`UiPlugin::manifest`] says where its UI goes:
//!   its pages, the points it declares, and its contributions to points,
//!   `tau-ui`'s ([`points`]) or other plugins'.

pub mod host;
pub mod manifest;
pub mod points;
pub mod registry;
pub mod run;
pub mod services;
#[cfg(any(test, feature = "testing"))]
pub mod testing;
pub mod value;
pub mod view;

use gpui::Context;
pub use host::{
    ConfigDir,
    HOST_RECORD,
    HostCx,
    HostRecord,
    Push,
    RepoCtx,
    RepoLauncher,
    RunCtx,
    RunKind,
    SavedSettings,
    TurnCommit,
    TurnHooks,
    WorkspaceDir,
};
pub use manifest::{
    ListedCommand,
    Manifest,
    Page,
    Point,
    PointCx,
    SlashCommand,
};
pub use registry::{CommandsAt, Env, ErasedPlugin, Registry};
pub use run::{
    CallData,
    CallResult,
    CardInfo,
    CardMark,
    Dropped,
    NESTED_TEXT_CHARS,
    NestedCall,
    NestedMark,
    OutputCut,
    RunCx,
    nested_under,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
pub use services::Services;
use tau_agent::plugin::Plugin;
pub use value::PluginValue;
pub use view::{
    Handle,
    Link,
    NavEntry,
    PlanField,
    PluginStatus,
    Request,
    RowNote,
    RunInfo,
    Sink,
    ViewCx,
};

/// A published body's key saying where history shows it. A run's history
/// stores a turn's messages when the turn ends, after what plugins
/// published during it; an interface replaying history places each body
/// as it showed live:
///
/// - none: with the turn it was published in, once the turn's tool
///   cards exist (a check on a call);
/// - [`PLACE_NOW`]: where it was stored, before the turn's messages (a
///   choice made before a request);
/// - [`PLACE_MESSAGE`]: after the message the run started on, which
///   history stores after what plugins published as the run started.
pub const PLACE: &str = "place";
pub const PLACE_NOW: &str = "now";
pub const PLACE_MESSAGE: &str = "message";

/// A run's history starts at its last context rewrite, and holds the
/// details the plugin gave it (`Rewrite::details`). Once the transcript
/// after it is in place, the plugin folds them as `{ "rewrite": details }`.
pub const REWRITE: &str = "rewrite";

/// `record`, placed in history at `place` ([`PLACE`]).
pub fn placed(record: &impl Serialize, place: &str) -> Value {
    let mut body =
        serde_json::to_value(record).expect("a plugin's record serializes");
    body[PLACE] = place.into();
    body
}

/// A plugin's state in a run, as the records it publishes fold into it,
/// live and from history alike.
pub trait Fold:
    Serialize + DeserializeOwned + Default + Clone + Send + Sync + 'static
{
    /// What the plugin publishes, one record at a time: the agent half
    /// writes it, this fold reads it. A record this fold cannot read is
    /// skipped, and said once.
    type Record: Serialize + DeserializeOwned + Send;

    fn apply(&mut self, record: Self::Record, run: &mut dyn RunCx);

    /// The details of the context rewrite a run's history starts at,
    /// once the transcript after it is in place ([`REWRITE`]).
    fn rewritten(&mut self, details: Value, run: &mut dyn RunCx) {
        let _ = (details, run);
    }
}

/// A plugin with no state in a run folds nothing.
impl Fold for () {
    type Record = Value;

    fn apply(&mut self, _: Value, _: &mut dyn RunCx) {}
}

/// What a plugin keeps on the host, made once from the host's context.
/// Any `Default` type is made by its default.
pub trait PluginHost: Send + Sync + Sized + 'static {
    fn new(cx: &HostCx) -> impl Future<Output = anyhow::Result<Self>> + Send;
}

impl<T: Default + Send + Sync + 'static> PluginHost for T {
    fn new(_: &HostCx) -> impl Future<Output = anyhow::Result<Self>> + Send {
        std::future::ready(Ok(Self::default()))
    }
}

/// What a plugin keeps per window, made in its own entity's context so
/// it can subscribe to what it makes (a field's Enter); `handle` is its
/// way back to that window. Any `Default` type is made by its default.
pub trait PluginUi: Sized + 'static {
    fn new(handle: Handle, cx: &mut Context<Self>) -> Self;
}

impl<T: Default + 'static> PluginUi for T {
    fn new(_: Handle, _: &mut Context<Self>) -> Self {
        Self::default()
    }
}

/// A plugin's record, the record type of its state's fold.
pub type RecordOf<P> = <<P as UiPlugin>::State as Fold>::Record;

/// What a plugin that asks Jev says of itself on the Plugins screen:
/// `what` it does, and that it needs a TypeSafe key when there is none.
pub fn needs_jev(jev: bool, what: &str) -> String {
    if jev {
        format!("{what}, with Jev")
    } else {
        format!("{what}: needs a TypeSafe key (Models)")
    }
}

/// What a plugin that asks Jev says in a run's plugin list when there is
/// no TypeSafe key.
pub const NO_KEY: &str = "off · no TypeSafe key";

/// The seams a plugin can use, in the order the loop reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seam {
    Start,
    Tools,
    BeforeTool,
    AfterTool,
    Rewrite,
    BeforeStop,
    Finish,
}

impl Seam {
    pub const ALL: [Self; 7] = [
        Self::Start,
        Self::Tools,
        Self::BeforeTool,
        Self::AfterTool,
        Self::Rewrite,
        Self::BeforeStop,
        Self::Finish,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Tools => "tools",
            Self::BeforeTool => "before_tool",
            Self::AfterTool => "after_tool",
            Self::Rewrite => "rewrite",
            Self::BeforeStop => "before_stop",
            Self::Finish => "finish",
        }
    }
}

/// A plugin's entry on the Plugins screen. The registry sets its name,
/// and the host its spend.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub description: String,
    pub seams: Vec<Seam>,
    /// What the plugin cost over the store's recent window; the host
    /// fills it in.
    pub spend: f64,
    /// The page that explains its work, if it has one.
    pub page: Option<Link>,
}

/// A plugin, with its UI.
pub trait UiPlugin: Sized + Send + Sync + 'static {
    /// What the plugin knows of one run, folded from what it publishes.
    type State: Fold;
    /// What it knows across repositories, from the host.
    type Data: Serialize
        + DeserializeOwned
        + Default
        + Clone
        + Send
        + Sync
        + 'static;
    /// What it knows of one repository, from the host.
    type RepoData: Serialize
        + DeserializeOwned
        + Default
        + Clone
        + Send
        + Sync
        + 'static;
    /// What the user set. The host saves it under the plugin's name.
    type Settings: Serialize
        + DeserializeOwned
        + Default
        + Clone
        + Send
        + Sync
        + 'static;
    /// What the plugin keeps on the host, made once.
    type Host: PluginHost;
    /// What it keeps per window: drafts, open tabs, a pending answer.
    type Ui: PluginUi;

    fn name(&self) -> &'static str;

    // On the machine that runs agents.

    /// The agent plugins for one run or sub-agent; none when the plugin
    /// is off for it. An error fails the run: what the plugin needs to
    /// check it could not be read.
    fn agent_plugins(
        &self,
        host: &Self::Host,
        run: &RunCtx,
        settings: &Self::Settings,
    ) -> impl Future<Output = anyhow::Result<Vec<Box<dyn Plugin>>>> + Send;

    /// Records to fold into a run's state as it starts or goes on, before
    /// anything is published: whether the plugin is on, and why not.
    fn starting(
        &self,
        _host: &Self::Host,
        _run: &RunCtx,
        _settings: &Self::Settings,
    ) -> impl Future<Output = Vec<RecordOf<Self>>> + Send {
        std::future::ready(Vec::new())
    }

    /// What agent commands in `repo` start through, when the plugin
    /// gives them an environment: `bash`'s commands, and the
    /// repository's MCP servers. None leaves them as they are; the host
    /// joins every plugin's in the registry's order
    /// ([`tau_agent::launch::Launchers`]).
    fn launcher(
        &self,
        _host: &Self::Host,
        _repo: &RepoCtx,
        _settings: &Self::Settings,
    ) -> impl Future<
        Output = Option<std::sync::Arc<dyn tau_agent::launch::Launcher>>,
    > + Send {
        std::future::ready(None)
    }

    /// Its entry on the Plugins screen. The registry sets its name.
    fn catalog(
        &self,
        host: &Self::Host,
        cx: &HostCx,
        settings: &Self::Settings,
    ) -> impl Future<Output = PluginInfo> + Send;

    fn data(
        &self,
        _host: &Self::Host,
        _cx: &HostCx,
    ) -> impl Future<Output = Self::Data> + Send {
        std::future::ready(Self::Data::default())
    }

    fn repo_data(
        &self,
        _host: &Self::Host,
        _repo: &RepoCtx,
        _cx: &HostCx,
    ) -> impl Future<Output = Self::RepoData> + Send {
        std::future::ready(Self::RepoData::default())
    }

    /// Carries out what the plugin's UI asked; a reply goes back to its
    /// [`Self::reply`].
    fn act(
        &self,
        _host: &Self::Host,
        _action: Value,
        _cx: &HostCx,
    ) -> impl Future<Output = anyhow::Result<Option<Value>>> + Send {
        std::future::ready(Ok(None))
    }

    // In the interface.

    /// What [`Self::act`] answered, in the window's UI state, which may
    /// ask the interface for more through its [`Handle`].
    fn reply(
        &self,
        _ui: &mut Self::Ui,
        _reply: Value,
        _cx: &mut Context<Self::Ui>,
    ) {
    }

    /// Whether its context rewrites keep the transcript, marking what
    /// they drop ([`RunCx::dropped`]), rather than replace it with
    /// something new (a summary).
    fn rewrites_keep_transcript(&self) -> bool {
        false
    }

    /// What a run on `prompt` is about, when the plugin reads it as its
    /// own command: for the run's title.
    fn read_prompt(&self, _prompt: &str) -> Option<String> {
        None
    }

    fn manifest(&self) -> Manifest<Self>;
}
