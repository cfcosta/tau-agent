//! What a plugin's UI reaches as it draws: its data, the run it draws
//! for, and a [`Handle`] back to the interface for what a click asks.

use std::{collections::BTreeMap, rc::Rc};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tau_agent::tool::RunId;
use tau_ui_kit::{assets::Icon, theme::Tone};

use crate::UiPlugin;

/// A run, as a contribution or a page sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunInfo {
    pub id: RunId,
    /// The repository it works in.
    pub repo: String,
    /// Whether it is going now.
    pub live: bool,
    /// Its title.
    pub title: String,
    /// The model's last text, once the run has finished: its answer.
    pub answer: Option<String>,
    /// The tokens in its context, and its model's window when known.
    pub context: u64,
    pub window: Option<u64>,
}

/// A page to open: a plugin's page and its parameters. A parameter
/// left empty is the one at hand where the link is followed: `run` the
/// run in view, `repo` the repository selected.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Link {
    /// The plugin whose page it is; `None` is the plugin making the link.
    pub plugin: Option<String>,
    pub page: String,
    pub params: BTreeMap<String, String>,
}

impl Link {
    /// The page `page` of the plugin making the link.
    pub fn page(page: impl Into<String>) -> Self {
        Self {
            plugin: None,
            page: page.into(),
            params: BTreeMap::new(),
        }
    }

    /// The page `page` of `plugin`.
    pub fn to(plugin: impl Into<String>, page: impl Into<String>) -> Self {
        Self {
            plugin: Some(plugin.into()),
            ..Self::page(page)
        }
    }

    pub fn param(
        mut self,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    /// This link, made by `plugin` when it names no plugin.
    pub fn from(mut self, plugin: &str) -> Self {
        self.plugin.get_or_insert_with(|| plugin.to_owned());
        self
    }
}

/// An entry in the interface's navigation: the sidebar, the phone's
/// lists, and search.
#[derive(Debug, Clone, PartialEq)]
pub struct NavEntry {
    pub label: String,
    pub icon: Icon,
    /// Small print beside the label: `12 notes`.
    pub detail: Option<String>,
    /// A count that wants attention: reviews waiting.
    pub badge: Option<(String, Tone)>,
    pub to: Link,
}

impl NavEntry {
    pub fn new(label: impl Into<String>, icon: Icon, to: Link) -> Self {
        Self {
            label: label.into(),
            icon,
            detail: None,
            badge: None,
            to,
        }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn badge(mut self, badge: Option<(String, Tone)>) -> Self {
        self.badge = badge;
        self
    }
}

/// A plugin's line in a run's plugin list: what it is doing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStatus {
    pub name: String,
    pub state: String,
    pub tone: Tone,
}

/// What a plugin adds to a run's row in the sidebar: a line under its
/// title, and small print at its end.
#[derive(Debug, Clone, PartialEq)]
pub struct RowNote {
    /// Under the title in the sidebar.
    pub line: Option<String>,
    /// Under the title in a phone's list, which has room for more.
    pub phone_line: Option<String>,
    /// At the row's end, after its icon.
    pub count: Option<String>,
    pub icon: Icon,
    pub tone: Tone,
}

/// A field of a run's plan, and who set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanField {
    pub name: String,
    pub value: String,
    pub set_by: Option<String>,
}

/// What a plugin's UI asks of the interface.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// The plugin's host half carries it out (`UiPlugin::act`).
    Act(Value),
    Navigate(Link),
    Alert {
        title: String,
        message: String,
    },
    /// Saves the plugin's settings: everywhere, or `repo`'s own copy.
    /// No value drops a repository's copy (ADR 0029).
    Settings {
        repo: Option<String>,
        value: Option<Value>,
    },
    /// Folds `body` into the run's state now, and stores it as the
    /// plugin's record with the run: a change the interface makes
    /// (pause a goal).
    Record {
        run: RunId,
        body: Value,
    },
    /// Sends `text` to a run going on.
    Steer {
        run: RunId,
        text: String,
    },
    /// Sends `text` as the next message: to `run`, or as a new run.
    Send {
        run: Option<RunId>,
        text: String,
    },
    /// Starts a run in the repository named `repo` with `text` as its
    /// first message, and opens it.
    Start {
        repo: String,
        text: String,
    },
    /// Puts `text` in the composer.
    Composer(String),
    /// Shows the run's details: the inspector, or a phone's sheet.
    RunDetails,
    /// Opens the pane of the run in view ([`crate::points::PANE`]).
    OpenPane,
    /// Sends what the composer holds, as Enter in it would.
    Submit,
    /// Draws the interface again.
    Refresh,
    /// Opens a run's conversation.
    OpenRun(RunId),
    /// Asks for the TypeSafe key plugins that ask Jev need.
    AskJevKey,
    /// Opens a page of the web in the person's browser.
    OpenUrl(String),
    /// Shows a file or folder in the system's file manager.
    Reveal(std::path::PathBuf),
    /// Opens a file with the system's application for it.
    Open(std::path::PathBuf),
    /// Puts text on the clipboard.
    Copy(String),
    /// Cancels a run going on, as its Cancel button does.
    Cancel(RunId),
    /// Shows `text` over the composer for a moment: what a command did
    /// when it started nothing where it was typed.
    Note(String),
    /// Ends the plugin's step of first-run setup ([`crate::points::SETUP`]):
    /// setup goes on to the next step, or to Ready.
    SetupDone,
    /// Sets `key` of the plugin's UI state, or forgets it: every
    /// interface of this tau shows the change, and the host keeps it
    /// (ADR 0045).
    Ui {
        key: String,
        value: Option<Value>,
    },
    /// Asks the plugin's host half to carry out `action`, whose answer
    /// is the picture `key` the interface keeps
    /// ([`crate::tree::Node::Image`]).
    Fetch {
        key: String,
        action: Value,
    },
}

/// Where a [`Handle`]'s requests go: the interface, which carries out
/// what `plugin` asked once the plugin's handler returns.
pub type Sink = Rc<dyn Fn(&'static str, Request)>;

/// A plugin's way back to the interface from what it carries out: an
/// action, a reply, a command or a key.
#[derive(Clone)]
pub struct Handle {
    plugin: &'static str,
    sink: Sink,
}

impl Handle {
    pub fn new(plugin: &'static str, sink: Sink) -> Self {
        Self { plugin, sink }
    }

    /// The plugin the handle speaks for.
    pub fn plugin(&self) -> &'static str {
        self.plugin
    }

    pub fn request(&self, request: Request) {
        (self.sink)(self.plugin, request);
    }

    /// Asks the plugin's host half for the picture or tree `key` with
    /// `action`; the answer goes to what the interface keeps of them.
    pub fn fetch(&self, key: String, action: Value) {
        self.request(Request::Fetch { key, action });
    }

    /// Sets `key` of the plugin's UI state to `value`, or forgets it.
    pub fn set_ui(&self, key: impl Into<String>, value: Option<Value>) {
        self.request(Request::Ui {
            key: key.into(),
            value,
        });
    }

    /// Asks the plugin's host half to carry out `action`.
    pub fn act(&self, action: impl Serialize) {
        let action = serde_json::to_value(action).unwrap_or_default();
        self.request(Request::Act(action));
    }

    pub fn navigate(&self, link: Link) {
        self.request(Request::Navigate(link.from(self.plugin)));
    }

    pub fn alert(&self, title: impl Into<String>, message: impl Into<String>) {
        self.request(Request::Alert {
            title: title.into(),
            message: message.into(),
        });
    }

    /// Saves the plugin's settings everywhere.
    pub fn save_settings(&self, settings: &impl Serialize) {
        self.save_settings_in(None, settings);
    }

    /// Saves the plugin's settings everywhere, or as `repo`'s own copy.
    pub fn save_settings_in(
        &self,
        repo: Option<&str>,
        settings: &impl Serialize,
    ) {
        let value = serde_json::to_value(settings).unwrap_or_default();
        self.request(Request::Settings {
            repo: repo.map(str::to_owned),
            value: Some(value),
        });
    }

    /// Drops `repo`'s own copy of the plugin's settings: its runs take
    /// the value everywhere again.
    pub fn drop_settings(&self, repo: &str) {
        self.request(Request::Settings {
            repo: Some(repo.to_owned()),
            value: None,
        });
    }

    pub fn record(&self, run: &RunId, body: impl Serialize) {
        let body = serde_json::to_value(body).unwrap_or_default();
        self.request(Request::Record {
            run: run.clone(),
            body,
        });
    }

    pub fn steer(&self, run: &RunId, text: impl Into<String>) {
        self.request(Request::Steer {
            run: run.clone(),
            text: text.into(),
        });
    }

    pub fn send(&self, run: Option<&RunId>, text: impl Into<String>) {
        self.request(Request::Send {
            run: run.cloned(),
            text: text.into(),
        });
    }

    pub fn start(&self, repo: impl Into<String>, text: impl Into<String>) {
        self.request(Request::Start {
            repo: repo.into(),
            text: text.into(),
        });
    }

    pub fn composer(&self, text: impl Into<String>) {
        self.request(Request::Composer(text.into()));
    }

    pub fn run_details(&self) {
        self.request(Request::RunDetails);
    }

    /// Opens the pane of the run in view ([`crate::points::PANE`]).
    pub fn open_pane(&self) {
        self.request(Request::OpenPane);
    }

    pub fn refresh(&self) {
        self.request(Request::Refresh);
    }

    /// Shows `text` over the composer for a moment.
    pub fn note(&self, text: impl Into<String>) {
        self.request(Request::Note(text.into()));
    }

    pub fn open_run(&self, run: &RunId) {
        self.request(Request::OpenRun(run.clone()));
    }

    /// Opens `url` in the person's browser: a sign-in.
    pub fn open_url(&self, url: impl Into<String>) {
        self.request(Request::OpenUrl(url.into()));
    }

    /// Shows `path` in the system's file manager: a folder the plugin
    /// keeps.
    pub fn reveal(&self, path: impl Into<std::path::PathBuf>) {
        self.request(Request::Reveal(path.into()));
    }

    /// Opens `path` with the system's application for it: a whole
    /// output kept on disk.
    pub fn open(&self, path: impl Into<std::path::PathBuf>) {
        self.request(Request::Open(path.into()));
    }

    /// Puts `text` on the clipboard.
    pub fn copy(&self, text: impl Into<String>) {
        self.request(Request::Copy(text.into()));
    }

    pub fn ask_jev_key(&self) {
        self.request(Request::AskJevKey);
    }

    pub fn cancel(&self, run: &RunId) {
        self.request(Request::Cancel(run.clone()));
    }

    /// Ends the plugin's step of first-run setup, the one on screen.
    pub fn setup_done(&self) {
        self.request(Request::SetupDone);
    }
}

/// A settings pane's parameter: the repository whose copy it shows.
pub const SCOPE: &str = "scope";

/// A settings pane's parameter: the plugin's own entry it is of.
pub const ENTRY: &str = "entry";

/// What a contribution, a page or a command reaches as it runs: the
/// plugin's values, the run, the page's parameters and UI state to read,
/// and a [`Handle`] for what an action, a command or a key asks. A view
/// only reads it, and gives back a tree (ADR 0045).
pub struct ViewCx<'a, P: UiPlugin> {
    pub plugin: &'a P,
    /// The plugin's state in the run drawn, when there is one.
    pub state: Option<&'a P::State>,
    pub data: &'a P::Data,
    pub settings: &'a P::Settings,
    /// The plugin's data for each repository, by name: see
    /// [`Self::repo`] and [`Self::repos`].
    repos: &'a BTreeMap<String, &'a crate::PluginValue>,
    /// The run drawn, when there is one.
    pub run: Option<&'a RunInfo>,
    /// A page's parameters; empty elsewhere.
    pub params: &'a BTreeMap<String, String>,
    /// Whether the window has the phone's layout.
    pub compact: bool,
    /// Whether a TypeSafe key is saved, so plugins that ask Jev run.
    pub jev: bool,
    /// The window's width, in pixels: where a table no longer fits.
    pub width: f32,
    pub handle: Handle,
    runs: &'a dyn Fn() -> Vec<(RunInfo, crate::PluginValue)>,
    cards: &'a dyn Fn(&RunId) -> Vec<crate::CardInfo>,
    /// Every plugin's data, by plugin: see [`Self::data_of`].
    others: &'a BTreeMap<String, crate::PluginValue>,
    /// The plugin's UI state: see [`Self::ui_value`].
    ui_state: &'a BTreeMap<String, Value>,
}

/// No plugin's data, for a view made without it.
static NO_OTHERS: BTreeMap<String, crate::PluginValue> = BTreeMap::new();
/// No UI state, for a view made without it.
static NO_UI_STATE: BTreeMap<String, Value> = BTreeMap::new();

impl<'a, P: UiPlugin> ViewCx<'a, P> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plugin: &'a P,
        state: Option<&'a P::State>,
        data: &'a P::Data,
        settings: &'a P::Settings,
        repos: &'a BTreeMap<String, &'a crate::PluginValue>,
        run: Option<&'a RunInfo>,
        params: &'a BTreeMap<String, String>,
        compact: bool,
        jev: bool,
        width: f32,
        handle: Handle,
        runs: &'a dyn Fn() -> Vec<(RunInfo, crate::PluginValue)>,
        cards: &'a dyn Fn(&RunId) -> Vec<crate::CardInfo>,
    ) -> Self {
        Self {
            plugin,
            state,
            data,
            settings,
            repos,
            run,
            params,
            compact,
            jev,
            width,
            handle,
            runs,
            cards,
            others: &NO_OTHERS,
            ui_state: &NO_UI_STATE,
        }
    }

    /// The same view, reading the plugin's UI state from `ui_state`.
    pub fn with_ui_state(
        mut self,
        ui_state: &'a BTreeMap<String, Value>,
    ) -> Self {
        self.ui_state = ui_state;
        self
    }

    /// `key` of the plugin's UI state, as every interface has it
    /// (ADR 0045); set it with [`Handle::set_ui`].
    pub fn ui_value(&self, key: &str) -> Option<&'a Value> {
        self.ui_state.get(key)
    }

    /// The plugin's whole UI state.
    pub fn ui_state(&self) -> &'a BTreeMap<String, Value> {
        self.ui_state
    }

    /// The same view, reading other plugins' data from `others`.
    pub fn with_others(
        mut self,
        others: &'a BTreeMap<String, crate::PluginValue>,
    ) -> Self {
        self.others = others;
        self
    }

    /// What the plugin `plugin` knows across repositories, as JSON: one
    /// plugin reads another's data without depending on its crate, so
    /// it reads only what it needs and takes none as nothing there.
    pub fn data_of(&self, plugin: &str) -> Option<&'a serde_json::Value> {
        self.others.get(plugin).map(crate::PluginValue::json)
    }

    /// The scope a settings pane shows: a repository's name, or none
    /// for everywhere.
    pub fn scope(&self) -> Option<&str> {
        self.params.get(SCOPE).map(String::as_str)
    }

    /// The entry a settings pane is of, for a plugin that lists its own
    /// (`PluginInfo::entries`).
    pub fn entry(&self) -> Option<&str> {
        self.params.get(ENTRY).map(String::as_str)
    }

    /// Saves `settings` in the scope the settings pane shows.
    pub fn save_settings(&self, settings: &P::Settings) {
        self.handle.save_settings_in(self.scope(), settings);
    }

    /// The tool calls of `run`, in order.
    pub fn cards(&self, run: &RunId) -> Vec<crate::CardInfo> {
        (self.cards)(run)
    }

    /// Every run the interface has, with the plugin's state in it.
    pub fn runs(&self) -> Vec<(RunInfo, P::State)> {
        (self.runs)()
            .into_iter()
            .map(|(run, state)| (run, state.get::<P::State>().clone()))
            .collect()
    }

    /// The plugin's data for the repository `name`.
    pub fn repo(&self, name: &str) -> Option<&'a P::RepoData> {
        self.repos.get(name).map(|value| value.get::<P::RepoData>())
    }

    /// The plugin's data for each repository, by name.
    pub fn repos(&self) -> impl Iterator<Item = (&'a str, &'a P::RepoData)> {
        self.repos
            .iter()
            .map(|(name, value)| (name.as_str(), value.get::<P::RepoData>()))
    }

    /// The page's parameter `name`, or nothing.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }
}
