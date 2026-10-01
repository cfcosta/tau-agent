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
//!   phone, [`UiPlugin::apply`] folds each body the plugin published into
//!   its state for the run, and places anchors in the transcript and on
//!   tool cards ([`RunCx`]).
//! - **In the interface**, [`UiPlugin::manifest`] says where its UI goes:
//!   its pages, the points it declares, and its contributions to points,
//!   `tau-ui`'s ([`points`]) or other plugins'.

pub mod host;
pub mod manifest;
pub mod points;
pub mod registry;
pub mod run;
pub mod services;
pub mod view;

use gpui::App;
pub use host::{HostCx, Push, RepoCtx, RunCtx, RunKind};
pub use manifest::{Manifest, Page, Point, PointCx, SlashCommand};
pub use registry::{Env, ErasedPlugin, Registry};
pub use run::{CardInfo, RunCx};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
pub use services::Services;
use tau_agent::plugin::Plugin;
pub use view::{
    Handle,
    Link,
    NavEntry,
    PlanField,
    PluginStatus,
    Request,
    RunInfo,
    Sink,
    ViewCx,
};

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

/// A plugin's entry on the Plugins screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// What the plugin knows of one run. Travels in the run's view as
    /// JSON.
    type State: Serialize + DeserializeOwned + Default + Clone + 'static;
    /// What it knows across repositories, from the host.
    type Data: Serialize + DeserializeOwned + Default + Clone + 'static;
    /// What it knows of one repository, from the host.
    type RepoData: Serialize + DeserializeOwned + Default + Clone + 'static;
    /// What the user set. The host saves it under the plugin's name.
    type Settings: Serialize + DeserializeOwned + Default + Clone + 'static;
    /// What the plugin keeps on the host, made once.
    type Host: Send + Sync + 'static;
    /// What it keeps per window: drafts, open tabs, a pending answer.
    type Ui: 'static;

    fn name(&self) -> &'static str;

    // On the machine that runs agents.

    fn host(&self, cx: &HostCx) -> anyhow::Result<Self::Host>;

    /// The agent plugins for one run or sub-agent; none when the plugin
    /// is off for it.
    fn agent_plugins(
        &self,
        host: &Self::Host,
        run: &RunCtx,
        settings: &Self::Settings,
    ) -> Vec<Box<dyn Plugin>>;

    /// Bodies to fold into a run's state as it starts or goes on, before
    /// anything is published: whether the plugin is on, and why not.
    fn starting(
        &self,
        _host: &Self::Host,
        _run: &RunCtx,
        _settings: &Self::Settings,
    ) -> Vec<Value> {
        Vec::new()
    }

    fn catalog(
        &self,
        host: &Self::Host,
        cx: &HostCx,
        settings: &Self::Settings,
    ) -> PluginInfo;

    fn data(&self, _host: &Self::Host, _cx: &HostCx) -> Self::Data {
        Self::Data::default()
    }

    fn repo_data(
        &self,
        _host: &Self::Host,
        _repo: &RepoCtx,
        _cx: &HostCx,
    ) -> Self::RepoData {
        Self::RepoData::default()
    }

    /// Carries out what the plugin's UI asked; a reply goes back to its
    /// [`Self::reply`].
    fn act(
        &self,
        _host: &Self::Host,
        _action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }

    // Wherever a run is shown.

    /// Folds one published body into the plugin's state in a run.
    fn apply(&self, state: &mut Self::State, body: &Value, run: &mut dyn RunCx);

    // In the interface.

    fn new_ui(&self, cx: &mut App) -> Self::Ui;

    /// What [`Self::act`] answered.
    fn reply(&self, _ui: &mut Self::Ui, _reply: Value) {}

    fn manifest(&self) -> Manifest<Self>;
}
