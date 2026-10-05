//! What a plugin's UI reaches as it draws: its data, the run it draws
//! for, and a [`Handle`] back to the interface for what a click asks.

use std::{collections::BTreeMap, rc::Rc};

use gpui::{App, Entity};
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
    /// Saves the plugin's settings.
    Settings(Value),
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
    /// Sends what the composer holds, as Enter in it would.
    Submit,
    /// Draws the interface again.
    Refresh,
    /// Opens a run's conversation.
    OpenRun(RunId),
    /// Asks for the TypeSafe key plugins that ask Jev need.
    AskJevKey,
    /// Gives the keys to `handle`, an element the plugin draws, as the
    /// window draws next: a panel that takes the composer's place. Not
    /// while the person is writing in the composer.
    Focus(gpui::FocusHandle),
    /// Cancels a run going on, as its Cancel button does.
    Cancel(RunId),
}

/// Where a [`Handle`]'s requests go: the interface, which carries out
/// what `plugin` asked.
pub type Sink = Rc<dyn Fn(&'static str, Request, &mut App)>;

/// A plugin's way back to the interface from an event handler.
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

    pub fn request(&self, request: Request, cx: &mut App) {
        (self.sink)(self.plugin, request, cx);
    }

    /// Asks the plugin's host half to carry out `action`.
    pub fn act(&self, action: impl Serialize, cx: &mut App) {
        let action = serde_json::to_value(action).unwrap_or_default();
        self.request(Request::Act(action), cx);
    }

    pub fn navigate(&self, link: Link, cx: &mut App) {
        self.request(Request::Navigate(link.from(self.plugin)), cx);
    }

    pub fn alert(
        &self,
        title: impl Into<String>,
        message: impl Into<String>,
        cx: &mut App,
    ) {
        self.request(
            Request::Alert {
                title: title.into(),
                message: message.into(),
            },
            cx,
        );
    }

    pub fn save_settings(&self, settings: &impl Serialize, cx: &mut App) {
        let settings = serde_json::to_value(settings).unwrap_or_default();
        self.request(Request::Settings(settings), cx);
    }

    pub fn record(&self, run: &RunId, body: impl Serialize, cx: &mut App) {
        let body = serde_json::to_value(body).unwrap_or_default();
        self.request(
            Request::Record {
                run: run.clone(),
                body,
            },
            cx,
        );
    }

    pub fn steer(&self, run: &RunId, text: impl Into<String>, cx: &mut App) {
        self.request(
            Request::Steer {
                run: run.clone(),
                text: text.into(),
            },
            cx,
        );
    }

    pub fn send(
        &self,
        run: Option<&RunId>,
        text: impl Into<String>,
        cx: &mut App,
    ) {
        self.request(
            Request::Send {
                run: run.cloned(),
                text: text.into(),
            },
            cx,
        );
    }

    pub fn start(
        &self,
        repo: impl Into<String>,
        text: impl Into<String>,
        cx: &mut App,
    ) {
        self.request(
            Request::Start {
                repo: repo.into(),
                text: text.into(),
            },
            cx,
        );
    }

    pub fn composer(&self, text: impl Into<String>, cx: &mut App) {
        self.request(Request::Composer(text.into()), cx);
    }

    pub fn run_details(&self, cx: &mut App) {
        self.request(Request::RunDetails, cx);
    }

    pub fn refresh(&self, cx: &mut App) {
        self.request(Request::Refresh, cx);
    }

    pub fn open_run(&self, run: &RunId, cx: &mut App) {
        self.request(Request::OpenRun(run.clone()), cx);
    }

    pub fn ask_jev_key(&self, cx: &mut App) {
        self.request(Request::AskJevKey, cx);
    }

    /// Gives the keys to `handle` as the window draws next.
    pub fn focus(&self, handle: &gpui::FocusHandle, cx: &mut App) {
        self.request(Request::Focus(handle.clone()), cx);
    }

    pub fn cancel(&self, run: &RunId, cx: &mut App) {
        self.request(Request::Cancel(run.clone()), cx);
    }
}

/// What a contribution, a page or a command reaches as it runs.
pub struct ViewCx<'a, P: UiPlugin> {
    pub plugin: &'a P,
    /// The plugin's state for this window: drafts, open tabs. Update it
    /// in a handler, then [`Handle::refresh`].
    pub ui: Entity<P::Ui>,
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
    pub cx: &'a mut App,
}

impl<'a, P: UiPlugin> ViewCx<'a, P> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        plugin: &'a P,
        ui: Entity<P::Ui>,
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
        cx: &'a mut App,
    ) -> Self {
        Self {
            plugin,
            ui,
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
            cx,
        }
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

    /// The theme.
    pub fn theme(&self) -> &tau_ui_kit::theme::Theme {
        tau_ui_kit::theme::theme(self.cx)
    }

    /// Reads the plugin's window state.
    pub fn read_ui(&self) -> &P::Ui {
        self.ui.read(self.cx)
    }
}
