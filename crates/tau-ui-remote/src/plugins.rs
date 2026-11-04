//! The plugins tau-ui has, each with its UI (ADR 0017), and how the
//! workspace draws them: what they contribute at its points, their
//! pages, and what their handles ask.

use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{LazyLock, OnceLock},
};

use gpui::{AnyElement, App, Context};
use serde_json::Value;
use tau_ui_plugin::{
    Env,
    ErasedPlugin,
    Handle,
    PluginValue,
    Point,
    PointCx,
    Registry,
    Request,
    RunInfo,
    points,
    wire::{Built, View, Wired},
};

use crate::{
    route::Route,
    view::RunView,
    workspace::{Workspace, WorkspaceEvent},
};

/// The plugins this process has: a host's, with their host halves, once
/// it has [`install`]ed them; until then, an interface's, as [`plugins`]
/// lists them. Both draw the same: only a host asks a plugin for what
/// its host half does, after installing them.
pub fn registry() -> &'static Registry {
    static INTERFACE: LazyLock<Registry> = LazyLock::new(plugins);
    INSTALLED.get().unwrap_or(&INTERFACE)
}

static INSTALLED: OnceLock<Registry> = OnceLock::new();

/// Whether a host installed its plugins.
pub fn installed() -> bool {
    INSTALLED.get().is_some()
}

/// Makes `registry` the plugins this process has: a host gives the ones
/// [`plugins`] lists their host halves (`Registry::host`) and installs
/// them before it asks any for its host state.
///
/// # Panics
///
/// When a registry was installed already: a process has one host.
pub fn install(registry: Registry) {
    if INSTALLED.set(registry).is_err() {
        panic!("the plugins were installed already");
    }
}

/// Every plugin, in the order the host adds them to a run's agent, with
/// its UI and without its host half (ADR 0030).
pub fn plugins() -> Registry {
    Registry::new()
        // The tools' cards: the host builds the tools themselves on
        // each run's workspace.
        .with(tau_tools::ui::ToolsUi)
        .with(tau_vcs::ui::VcsUi)
        .with(tau_reasoning::ReasoningPlugin)
        // Pruning first: it is cheaper than a summary, and summarizing
        // follows when pruning cannot help. The tree, when switched on,
        // folds before the summary, which steps in when the tree fails.
        .with(tau_fast_compaction::ui::FastCompactionUi)
        .with(tau_tree_compaction::ui::TreeCompactionUi)
        .with(tau_compaction::ui::CompactionUi)
        // The repository's rules check what the tools do; a goal's
        // hold of a stop comes after theirs.
        // Notes for what the tools do, and the repository's rules to
        // check it.
        .with(tau_memory::ui::MemoryUi)
        .with(tau_constitution::ui::ConstitutionUi)
        .with(tau_goal::GoalUi)
        // Notes on what the person likely missed, once switched on.
        .with(tau_watcher::WatcherUi)
        // Plugins the person wrote in Luau: after the rules and the
        // goal, so theirs hold first.
        .with(tau_luau_plugins::LuauPluginsUi)
        // Agents: their pages, and what their conversations show.
        .with(tau_agents::AgentsUi)
        // A chat's wake-ups, which it schedules for itself.
        .with(tau_schedule::ScheduleUi)
        // Questions the agent asks the person, answered in the
        // composer's place.
        .with(tau_ask::AskUi)
        // What the agent shows the person: images and documents, in a
        // pane beside the run.
        .with(tau_canvas::CanvasUi)
        // Agent commands in the repository's direnv environment, once
        // the person allows it.
        .with(tau_direnv::DirenvUi)
        // MCP servers' tools: their direct ones are added to the run's
        // plan in tau-mcp's `start`, so it comes before Codemode.
        .with(tau_mcp::McpUi)
        // Skills the model loads when a task calls for them.
        .with(tau_skills::SkillsUi)
        // Codemode's `start` lists the Luau signatures of the tools in
        // the run's plan when it runs. A plugin that adds tools in its
        // own `start` (tau-mcp, for its servers' direct tools) must be
        // registered before it, or scripts' signatures miss them.
        .with(tau_codemode::CodemodeUi)
}

/// What a run on `prompt` is about, as the first plugin that reads it as
/// its own command says: `/goal the tests pass` is about the tests
/// passing.
pub fn read_prompt(prompt: &str) -> Option<String> {
    registry()
        .plugins()
        .find_map(|plugin| plugin.read_prompt(prompt))
}

/// The points tau-ui declares.
pub const DECLARED: [&str; points::ALL.len()] = points::ALL;

/// A requests queue a plugin's handle fills, drained after the event
/// that filled it: a handle may ask from inside the workspace's own
/// update.
pub(crate) type Requests = Rc<std::cell::RefCell<Vec<(&'static str, Request)>>>;

/// A handle for `plugin` that asks through `queue`: what it asks is
/// carried out once the plugin's handler returns
/// ([`Workspace::drain_plugin_requests`]).
pub(crate) fn handle_for(plugin: &'static str, queue: Requests) -> Handle {
    Handle::new(
        plugin,
        Rc::new(move |plugin, request| {
            queue.borrow_mut().push((plugin, request))
        }),
    )
}

/// What the window's own parts of a plugin's tree (an input, a widget)
/// ask for it, as they draw or take an event: what its handle asks, or
/// the keys for an element of theirs.
pub enum Ask {
    Plugin(Request),
    /// Gives the keys to an element as the window draws next: a panel
    /// that takes the composer's place. Not while the person is writing
    /// in the composer.
    Focus(gpui::FocusHandle),
}

/// Where an [`Asker`]'s asks go: the workspace, which carries out what
/// `plugin`'s tree asked.
pub type AskSink = Rc<dyn Fn(&'static str, Ask, &mut App)>;

/// The way back to the workspace for the window's own parts of a
/// plugin's tree: what they ask is carried out after the event that
/// asked.
#[derive(Clone)]
pub struct Asker {
    plugin: &'static str,
    ask: AskSink,
}

impl Asker {
    pub fn new(plugin: &'static str, ask: AskSink) -> Self {
        Self { plugin, ask }
    }

    /// The plugin it asks for.
    pub fn plugin(&self) -> &'static str {
        self.plugin
    }

    pub fn request(&self, request: Request, cx: &mut App) {
        (self.ask)(self.plugin, Ask::Plugin(request), cx);
    }

    /// Asks the plugin's host half for the picture or tree `key`.
    pub fn fetch(&self, key: String, action: Value, cx: &mut App) {
        self.request(Request::Fetch { key, action }, cx);
    }

    /// Sets `key` of the plugin's UI state, or forgets it.
    pub fn set_ui(&self, key: String, value: Option<Value>, cx: &mut App) {
        self.request(Request::Ui { key, value }, cx);
    }

    /// Draws the window again.
    pub fn refresh(&self, cx: &mut App) {
        self.request(Request::Refresh, cx);
    }

    /// Gives the keys to `focus` as the window draws next.
    pub fn focus(&self, focus: &gpui::FocusHandle, cx: &mut App) {
        (self.ask)(self.plugin, Ask::Focus(focus.clone()), cx);
    }
}

/// Where a plugin's tree was drawn: the run and the page's parameters
/// its actions are carried out with ([`Workspace::plugin_action`]).
#[derive(
    Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize,
)]
pub struct Origin {
    pub run: Option<tau_agent::tool::RunId>,
    pub params: BTreeMap<String, String>,
}

/// What a phone asks the computer to carry out for a plugin (ADR 0046).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "call", rename_all = "snake_case")]
pub enum PluginCall {
    /// A button, a switch, an input's Enter or a key in its tree.
    Action { action: String, args: Value },
    /// A slash command typed in the composer, in `repo`.
    Command {
        name: String,
        args: String,
        repo: Option<String>,
    },
    /// A key it binds, pressed in `repo`.
    Key { key: String, repo: Option<String> },
}

/// A new call's id: apart from every other phone's, without a source of
/// randomness of its own.
fn call_id() -> u64 {
    use std::{
        hash::{BuildHasher as _, Hasher as _},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut hasher =
        std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(NEXT.fetch_add(1, Ordering::Relaxed));
    hasher.finish()
}

impl Origin {
    /// Where a contribution at `at` is drawn.
    pub fn at(at: &impl PointCx) -> Self {
        Self {
            run: at.run().map(|run| run.id.clone()),
            params: BTreeMap::new(),
        }
    }
}

impl Workspace {
    /// The handle `plugin`'s UI asks the workspace through.
    pub fn plugin_handle(&self, plugin: &'static str) -> Handle {
        handle_for(plugin, self.plugin_requests.clone())
    }

    /// The asker the window's own parts of `plugin`'s trees ask through.
    pub(crate) fn plugin_asker(&self, plugin: &'static str) -> Asker {
        let (queue, workspace) =
            (self.plugin_requests.clone(), self.weak.clone());
        Asker::new(
            plugin,
            Rc::new(move |plugin, ask, cx: &mut App| {
                let workspace = workspace.clone();
                match ask {
                    Ask::Plugin(request) => {
                        queue.borrow_mut().push((plugin, request));
                        cx.defer(move |cx| {
                            let _ = workspace.update(cx, |ws, cx| {
                                ws.drain_plugin_requests(cx)
                            });
                        });
                    }
                    Ask::Focus(focus) => cx.defer(move |cx| {
                        let _ = workspace.update(cx, |ws, cx| {
                            ws.plugin_focus = Some(focus);
                            cx.notify();
                        });
                    }),
                }
            }),
        )
    }

    /// Sets `key` of `plugin`'s UI state, as its trees do: where the
    /// demo and tests put a window.
    pub fn set_plugin_ui(
        &mut self,
        plugin: &'static str,
        key: impl Into<String>,
        value: Option<Value>,
        cx: &mut Context<Self>,
    ) {
        let key = key.into();
        self.plugin_request(plugin, Request::Ui { key, value }, cx);
    }

    /// Carries out what plugins' handles asked.
    pub fn drain_plugin_requests(&mut self, cx: &mut Context<Self>) {
        let requests: Vec<_> =
            self.plugin_requests.borrow_mut().drain(..).collect();
        for (plugin, request) in requests {
            self.plugin_request(plugin, request, cx);
        }
    }

    fn plugin_request(
        &mut self,
        plugin: &'static str,
        request: Request,
        cx: &mut Context<Self>,
    ) {
        match request {
            Request::Act(action) => cx.emit(WorkspaceEvent::PluginAct {
                plugin: plugin.to_owned(),
                action,
                asker: self.asker,
            }),
            Request::Navigate(link) => {
                let link = link.from(plugin);
                self.navigate(
                    Route::Plugin {
                        plugin: link.plugin.unwrap_or_default(),
                        page: link.page,
                        params: link.params,
                    },
                    cx,
                );
            }
            Request::Alert { title, message } => {
                self.show_alert(title, message, cx);
            }
            Request::Settings { repo, value } => {
                match (&repo, &value) {
                    (None, Some(value)) => {
                        self.catalog.plugin_settings.insert(
                            plugin.to_owned(),
                            PluginValue::from_json(value.clone()),
                        );
                    }
                    (Some(repo), Some(value)) => {
                        self.catalog
                            .repo_plugin_settings
                            .entry(repo.clone())
                            .or_default()
                            .insert(
                                plugin.to_owned(),
                                PluginValue::from_json(value.clone()),
                            );
                    }
                    (Some(repo), None) => {
                        if let Some(plugins) =
                            self.catalog.repo_plugin_settings.get_mut(repo)
                        {
                            plugins.remove(plugin);
                        }
                    }
                    (None, None) => return,
                }
                cx.emit(WorkspaceEvent::PluginSettings {
                    plugin: plugin.to_owned(),
                    repo,
                    settings: value,
                });
            }
            // It folds once the host stores it (`HostUpdate::PluginRecord`),
            // on every interface alike.
            Request::Record { run, body } => {
                cx.emit(WorkspaceEvent::PluginRecord {
                    run,
                    plugin: plugin.to_owned(),
                    body,
                });
            }
            Request::Steer { run, text } => self.say(&run, text, cx),
            Request::Send { run, text } => match run {
                Some(run) => self.say(&run, text, cx),
                None => self.send(text, cx),
            },
            Request::Start { repo, text } => {
                cx.emit(WorkspaceEvent::NewRun {
                    prompt: text,
                    model: self.next_model.clone(),
                    repo,
                    images: Vec::new(),
                });
            }
            Request::Composer(text) => {
                // From a page with no composer, to a new run's.
                if !matches!(
                    self.route,
                    Route::Run(_) | Route::Home | Route::NewRun
                ) {
                    self.navigate(Route::NewRun, cx);
                }
                self.composer
                    .update(cx, |input, cx| input.set_text(text, cx));
                self.composer_changed(cx);
            }
            Request::RunDetails => {
                if self.compact() && !self.sheet_is_open() {
                    self.toggle_sheet(cx);
                }
            }
            Request::OpenPane => self.open_pane(cx),
            Request::Submit => self.submit_from_button(cx),
            Request::Refresh => {}
            Request::OpenRun(run) => self.navigate(Route::Run(run), cx),
            Request::AskJevKey => self.ask_for_jev_key_later(cx),
            Request::OpenUrl(url) => cx.open_url(&url),
            Request::Reveal(path) => cx.reveal_path(&path),
            Request::Open(path) => cx.open_with_system(&path),
            Request::Copy(text) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(text))
            }
            Request::Cancel(run) => cx.emit(WorkspaceEvent::Cancel { run }),
            Request::Note(text) => self.show_composer_note(text, cx),
            Request::SetupDone => self.plugin_setup_step_done(plugin, cx),
            Request::Fetch { key, action } => {
                cx.emit(WorkspaceEvent::PluginFetch {
                    plugin: plugin.to_owned(),
                    key,
                    action,
                })
            }
            Request::Ui { key, value } => {
                // Shown here at once; the host keeps it, and tells the
                // other interfaces.
                let event = tau_ui_plugin::UiEvent {
                    plugin: plugin.to_owned(),
                    key,
                    value,
                };
                self.ui_state.apply(&event);
                cx.emit(WorkspaceEvent::Ui(event));
            }
        }
        cx.notify();
    }

    /// What a plugin's answer to an action says, for its UI.
    pub fn plugin_reply(
        &mut self,
        plugin: &str,
        reply: Value,
        asker: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        // The computer answers what a phone's call asked (ADR 0046).
        if self.draws_remote_views() {
            return;
        }
        if let Some(erased) = registry().get(plugin) {
            erased.reply(reply, &self.plugin_handle(erased.name()));
            self.carry_out_for(erased.name(), asker, cx);
        }
        cx.notify();
    }

    /// Carries out what `plugin`'s handle asked, for the phone's call
    /// `asker` when one asked: what is for its window goes back to it
    /// ([`HostUpdate::Called`]); the rest is carried out here.
    fn carry_out_for(
        &mut self,
        plugin: &'static str,
        asker: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = asker else {
            self.drain_plugin_requests(cx);
            return;
        };
        let requests: Vec<(&'static str, Request)> =
            self.plugin_requests.borrow_mut().drain(..).collect();
        let mut windows = Vec::new();
        self.asker = Some(id);
        for (by, request) in requests {
            if by == plugin && request.for_window() {
                windows.push(request);
            } else {
                self.plugin_request(by, request, cx);
            }
        }
        self.asker = None;
        if !windows.is_empty() {
            // Applied here, it reaches the phones through the mirror.
            self.apply(
                crate::update::HostUpdate::Called {
                    id,
                    plugin: plugin.to_owned(),
                    requests: windows,
                },
                cx,
            );
        }
    }

    /// Asks the computer to carry out `call` for `plugin`, where this
    /// phone drew it: what is for this window comes back.
    fn call_computer(
        &mut self,
        plugin: &str,
        origin: Origin,
        call: PluginCall,
        cx: &mut Context<Self>,
    ) {
        let id = call_id();
        self.calls.insert(id);
        cx.emit(WorkspaceEvent::PluginCall {
            id,
            plugin: plugin.to_owned(),
            origin,
            layout: self.view_layout(),
            call,
        });
    }

    /// Carries out a phone's call `id` for `plugin`, as the phone drew it
    /// at `origin` in `layout` (ADR 0046).
    pub fn answer_call(
        &mut self,
        id: u64,
        plugin: &str,
        origin: &Origin,
        layout: tau_ui_plugin::wire::Layout,
        call: &PluginCall,
        cx: &mut Context<Self>,
    ) {
        let Some(erased) = registry().get(plugin) else {
            return;
        };
        let run = origin.run.as_ref().and_then(|run| self.run(run)).cloned();
        self.with_plugin_in(
            erased.as_ref(),
            run.as_ref(),
            &origin.params,
            Some(layout),
            |plugin, env| match call {
                PluginCall::Action { action, args } => {
                    plugin.on_action(action, args, env)
                }
                PluginCall::Command { name, args, repo } => {
                    plugin.run_command(name, args, repo.as_deref(), env);
                }
                PluginCall::Key { key, repo } => {
                    let at = tau_ui_plugin::KeyAt { repo: repo.clone() };
                    plugin.press_key(key, &at, env);
                }
            },
        );
        self.carry_out_for(erased.name(), Some(id), cx);
        cx.notify();
    }

    /// Carries out what the computer sent back for this phone's call
    /// `id`; another phone's is not this window's.
    pub(crate) fn called_back(
        &mut self,
        id: u64,
        plugin: &str,
        requests: Vec<Request>,
        cx: &mut Context<Self>,
    ) {
        if !self.calls.remove(&id) {
            return;
        }
        let Some(erased) = registry().get(plugin) else {
            return;
        };
        for request in requests {
            self.plugin_request(erased.name(), request, cx);
        }
    }

    /// Runs `f` with what `plugin` draws from: its values, the run when
    /// there is one, and the page's parameters.
    pub(crate) fn with_plugin<R>(
        &self,
        plugin: &dyn ErasedPlugin,
        run: Option<&RunView>,
        params: &BTreeMap<String, String>,
        f: impl FnOnce(&dyn ErasedPlugin, Env<'_>) -> R,
    ) -> Option<R> {
        self.with_plugin_in(plugin, run, params, None, f)
    }

    /// [`Self::with_plugin`], laid out as `layout` says rather than as
    /// this window is: a phone's view, built here (ADR 0046).
    pub(crate) fn with_plugin_in<R>(
        &self,
        plugin: &dyn ErasedPlugin,
        run: Option<&RunView>,
        params: &BTreeMap<String, String>,
        layout: Option<tau_ui_plugin::wire::Layout>,
        f: impl FnOnce(&dyn ErasedPlugin, Env<'_>) -> R,
    ) -> Option<R> {
        let name = plugin.name();
        let info = run.map(RunView::info);
        // Each value the plugin has not got yet is unset, and typed apart
        // once read.
        let (no_state, no_data, no_settings): (
            PluginValue,
            PluginValue,
            PluginValue,
        ) = Default::default();
        let state = run.and_then(|run| run.plugin_states.get(name));
        let data = self.catalog.plugin_data.get(name).unwrap_or(&no_data);
        // A settings pane shows its scope's value; everything else the
        // value everywhere.
        let scope = params.get(tau_ui_plugin::SCOPE).map(String::as_str);
        let settings = self
            .catalog
            .settings_in(name, scope)
            .unwrap_or(&no_settings);
        let repos: BTreeMap<String, &PluginValue> = self
            .catalog
            .repos
            .iter()
            .filter_map(|repo| {
                Some((repo.name.clone(), repo.plugins.get(name)?))
            })
            .collect();
        let runs = || -> Vec<(RunInfo, PluginValue)> {
            self.runs
                .iter()
                .map(|run| {
                    (
                        run.info(),
                        run.plugin_states
                            .get(name)
                            .cloned()
                            .unwrap_or_default(),
                    )
                })
                .collect()
        };
        let cards =
            |id: &tau_agent::tool::RunId| -> Vec<tau_ui_plugin::CardInfo> {
                self.runs
                    .iter()
                    .find(|run| &run.id == id)
                    .map(RunView::cards)
                    .unwrap_or_default()
            };
        let env = Env {
            state: state.or(run.map(|_| &no_state)),
            data,
            settings,
            repos: &repos,
            run: info.as_ref(),
            params,
            compact: layout
                .map_or_else(|| self.compact(), |layout| layout.compact),
            jev: self.catalog.models.access.jev,
            width: layout.map_or_else(
                || f32::from(self.screen_width()),
                |layout| layout.width(),
            ),
            handle: self.plugin_handle(name),
            runs: &runs,
            cards: &cards,
            others: &self.catalog.plugin_data,
            ui_state: self.ui_state.of(name),
        };
        Some(f(plugin, env))
    }

    /// Draws `node`, a tree `plugin` gave, named `id` apart from other
    /// trees on screen (ADR 0045): with the plugin's UI state, the
    /// window's inputs, widgets and fetched trees, and its clicks carried
    /// out. A link opens its page, a section or a tab changes the UI
    /// state, and the plugin's own actions go to its `on_action` with
    /// the run and parameters of `origin`.
    pub fn plugin_tree(
        &self,
        plugin: &'static str,
        id: &str,
        node: &tau_ui_plugin::tree::Node,
        origin: Origin,
        cx: &mut App,
    ) -> AnyElement {
        use tau_ui_plugin::tree::{Act, Draw};

        let asker = self.plugin_asker(plugin);
        let tree_inputs = self.tree_inputs.clone();
        let inputs = {
            let asker = asker.clone();
            move |spec: tau_ui_plugin::tree::InputSpec, cx: &mut App| {
                crate::tree_inputs::editor(&tree_inputs, &asker, spec, cx)
            }
        };
        let tree_widgets = self.tree_widgets.clone();
        let widgets = {
            let (tree_widgets, asker) = (tree_widgets.clone(), asker.clone());
            move |spec: tau_ui_plugin::tree::WidgetSpec, cx: &mut App| {
                crate::tree_widgets::widget(&tree_widgets, &asker, spec, cx)
            }
        };
        let fetches = move |key: &str, fetch: &Value, cx: &mut App| {
            crate::tree_widgets::tree(&tree_widgets, &asker, key, fetch, cx)
        };
        let workspace = self.weak.clone();
        let on_act = Rc::new(move |act: Act, cx: &mut App| {
            let (workspace, origin) = (workspace.clone(), origin.clone());
            cx.defer(move |cx| {
                let _ = workspace.update(cx, |ws, cx| match act {
                    Act::Navigate(link) => {
                        ws.plugin_request(plugin, Request::Navigate(link), cx)
                    }
                    Act::Ui { key, value } => ws.plugin_request(
                        plugin,
                        Request::Ui { key, value },
                        cx,
                    ),
                    Act::Plugin { action, args } => {
                        ws.plugin_action(plugin, &action, &args, &origin, cx)
                    }
                });
            });
        });
        let t = tau_ui_kit::theme::theme(cx).clone();
        Draw::new(&t, id.to_owned())
            .width(f32::from(self.screen_width()))
            .window((f32::from(self.width), f32::from(self.height)))
            .ui(self.ui_state.of(plugin))
            .on_act(on_act)
            .inputs(&inputs)
            .widgets(&widgets)
            .fetches(&fetches)
            .draw(node, cx)
    }

    /// Carries out `plugin`'s own `action` from one of its trees, drawn
    /// at `origin`: the plugin reads its values again, and what it asks
    /// is carried out after.
    pub fn plugin_action(
        &mut self,
        plugin: &str,
        action: &str,
        args: &Value,
        origin: &Origin,
        cx: &mut Context<Self>,
    ) {
        if self.draws_remote_views() {
            let call = PluginCall::Action {
                action: action.to_owned(),
                args: args.clone(),
            };
            self.call_computer(plugin, origin.clone(), call, cx);
            return;
        }
        let Some(erased) = registry().get(plugin) else {
            return;
        };
        let run = origin
            .run
            .as_ref()
            .and_then(|id| self.runs.iter().find(|run| &run.id == id))
            .cloned();
        self.with_plugin(
            erased.as_ref(),
            run.as_ref(),
            &origin.params,
            |plugin, env| plugin.on_action(action, args, env),
        );
        self.drain_plugin_requests(cx);
        cx.notify();
    }

    /// What plugins contribute at `point`, drawn: each tree in order, by
    /// each contribution's order, then the registry's.
    pub fn drawn<Cx: PointCx>(
        &self,
        point: Point<Cx, tau_ui_plugin::tree::Node>,
        at: &Cx,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.drawn_with(point, |_| Some(at), cx)
    }

    /// What `plugin` alone contributes at `point`, drawn.
    pub fn drawn_of<Cx: PointCx>(
        &self,
        plugin: &str,
        point: Point<Cx, tau_ui_plugin::tree::Node>,
        at: &Cx,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        self.drawn_with(point, |name| (name == plugin).then_some(at), cx)
    }

    /// What plugins contribute at `point`, each with the context `at`
    /// gives it, drawn in order.
    pub fn drawn_with<'c, Cx: PointCx>(
        &self,
        point: Point<Cx, tau_ui_plugin::tree::Node>,
        at: impl Fn(&str) -> Option<&'c Cx>,
        cx: &mut App,
    ) -> Vec<AnyElement> {
        let found = self.contributions_named(point, &at);
        found
            .into_iter()
            .enumerate()
            .filter_map(|(n, (plugin, node))| {
                let at = at(plugin)?;
                let id = format!("{plugin}-{}-{}-{n}", point.name, at.key());
                Some(self.plugin_tree(plugin, &id, &node, Origin::at(at), cx))
            })
            .collect()
    }

    /// What plugins contribute at `point`, in order: by each
    /// contribution's order, then the registry's.
    pub fn contributions<Cx: PointCx, Out: Wired>(
        &self,
        point: Point<Cx, Out>,
        at: &Cx,
    ) -> Vec<Out> {
        self.contributions_with(point, |_| Some(at))
    }

    /// What `plugin` alone contributes at `point`: at one of its anchors.
    pub fn contributions_of<Cx: PointCx, Out: Wired>(
        &self,
        plugin: &str,
        point: Point<Cx, Out>,
        at: &Cx,
    ) -> Vec<Out> {
        self.contributions_with(point, |name| (name == plugin).then_some(at))
    }

    /// What plugins contribute at `point`, each with the context `at`
    /// gives it (none skips the plugin), in order.
    pub fn contributions_with<'c, Cx: PointCx, Out: Wired>(
        &self,
        point: Point<Cx, Out>,
        at: impl Fn(&str) -> Option<&'c Cx>,
    ) -> Vec<Out> {
        self.contributions_named(point, at)
            .into_iter()
            .map(|(_, out)| out)
            .collect()
    }

    /// What plugins contribute at `point`, in order, each with the plugin
    /// that gave it.
    pub fn contributions_by<Cx: PointCx, Out: Wired>(
        &self,
        point: Point<Cx, Out>,
        at: &Cx,
    ) -> Vec<(&'static str, Out)> {
        self.contributions_named(point, |_| Some(at))
    }

    /// What plugins contribute at `point`, in order, each with the plugin
    /// that gave it and the context `at` gave it.
    pub fn contributions_named<'c, Cx: PointCx, Out: Wired>(
        &self,
        point: Point<Cx, Out>,
        at: impl Fn(&str) -> Option<&'c Cx>,
    ) -> Vec<(&'static str, Out)> {
        self.contributions_in(point, at, None)
    }

    /// [`Self::contributions_named`], laid out as `layout` says.
    pub(crate) fn contributions_in<'c, Cx: PointCx, Out: Wired>(
        &self,
        point: Point<Cx, Out>,
        at: impl Fn(&str) -> Option<&'c Cx>,
        layout: Option<tau_ui_plugin::wire::Layout>,
    ) -> Vec<(&'static str, Out)> {
        // A phone draws what the computer built (ADR 0046).
        if layout.is_none()
            && let Some(outs) = self.remote_contributions(point.name, &at)
        {
            return outs;
        }
        let empty = BTreeMap::new();
        let mut found: Vec<(i32, usize, &'static str, Out)> = Vec::new();
        for (n, plugin) in registry().plugins().enumerate() {
            let Some(at) = at(plugin.name()) else {
                continue;
            };
            let run = at.run().and_then(|info| {
                self.runs.iter().find(|run| run.id == info.id)
            });
            let outs = self
                .with_plugin_in(
                    plugin.as_ref(),
                    run,
                    &empty,
                    layout,
                    |plugin, env| plugin.contribute(point.name, at, env),
                )
                .unwrap_or_default();
            for (order, mut out) in outs {
                // A plugin's navigation entry leads to its own page unless
                // it names another plugin's.
                if let Some(entry) =
                    out.downcast_mut::<tau_ui_plugin::NavEntry>()
                    && entry.to.plugin.is_none()
                {
                    entry.to.plugin = Some(plugin.name().to_owned());
                }
                if let Ok(out) = out.downcast::<Out>() {
                    found.push((order, n, plugin.name(), *out));
                }
            }
        }
        found.sort_by_key(|(order, n, _, _)| (*order, *n));
        found
            .into_iter()
            .map(|(_, _, plugin, out)| (plugin, out))
            .collect()
    }

    /// What plugins contribute at `point`, as the computer built it for
    /// this window; none on a window that builds its own.
    fn remote_contributions<'c, Cx: PointCx, Out: Wired>(
        &self,
        point: &'static str,
        at: &impl Fn(&str) -> Option<&'c Cx>,
    ) -> Option<Vec<(&'static str, Out)>> {
        if !self.draws_remote_views() {
            return None;
        }
        // Every plugin's context names the same place.
        let place =
            registry().plugins().find_map(|plugin| at(plugin.name()))?;
        let Some(place) = place.wire() else {
            return Some(Vec::new());
        };
        let view = View::Point {
            point: point.to_owned(),
            at: place,
        };
        let Some(Some(Built::Outs { outs })) = self.remote_view(view) else {
            return Some(Vec::new());
        };
        let outs = outs
            .into_iter()
            .filter(|out| at(&out.plugin).is_some())
            .filter_map(|out| {
                let plugin = registry().get(&out.plugin)?.name();
                Some((plugin, Out::from_wire(out.out)?))
            })
            .collect();
        Some(outs)
    }

    /// `plugin`'s page `page`, drawn with `params`.
    pub(crate) fn plugin_page(
        &self,
        plugin: &str,
        page: &str,
        params: &BTreeMap<String, String>,
        cx: &mut App,
    ) -> Option<AnyElement> {
        use gpui::{
            InteractiveElement as _,
            IntoElement as _,
            ParentElement as _,
            StatefulInteractiveElement as _,
            Styled as _,
        };
        use tau_ui_kit::theme::sp;

        let erased = registry().get(plugin)?;
        let run = params.get("run").and_then(|id| {
            self.runs.iter().find(|run| &*run.id.0 == id.as_str())
        });
        let remote = self.remote_view(View::Page {
            plugin: erased.name().to_owned(),
            page: page.to_owned(),
            params: params.clone(),
        });
        let (tree, framed) = match remote {
            Some(Some(Built::Page { tree, framed })) => (tree, framed),
            Some(_) => return None,
            None => self
                .with_plugin(erased.as_ref(), run, params, |plugin, env| {
                    plugin.draw_page(page, env)
                })
                .flatten()?,
        };
        let origin = Origin {
            run: run.map(|run| run.id.clone()),
            params: params.clone(),
        };
        let id = format!("{}-page-{page}", erased.name());
        let drawn = self.plugin_tree(erased.name(), &id, &tree, origin, cx);
        if !framed {
            return Some(drawn);
        }
        // The interface frames the page: it scrolls, with a page's
        // margins on the computer and the phone.
        let compact = self.compact();
        Some(
            gpui::div()
                .id(gpui::SharedString::from(format!("page-{page}")))
                .flex_1()
                .min_h(gpui::rems(0.))
                .overflow_y_scroll()
                .child(
                    gpui::div()
                        .flex()
                        .flex_col()
                        .gap(sp(4.5))
                        .px(sp(if compact { 4. } else { 8. }))
                        .py(sp(if compact { 4. } else { 6. }))
                        .child(drawn),
                )
                .into_any_element(),
        )
    }

    /// `plugin`'s settings pane, for `scope` (a repository, or none for
    /// everywhere) and, for a plugin that lists its own, `entry`.
    pub(crate) fn plugin_settings_pane(
        &self,
        plugin: &str,
        scope: Option<&str>,
        entry: Option<&str>,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let erased = registry().get(plugin)?;
        let mut params = BTreeMap::new();
        if let Some(scope) = scope {
            params.insert(tau_ui_plugin::SCOPE.to_owned(), scope.to_owned());
        }
        if let Some(entry) = entry {
            params.insert(tau_ui_plugin::ENTRY.to_owned(), entry.to_owned());
        }
        let remote = self.remote_view(View::Settings {
            plugin: erased.name().to_owned(),
            scope: scope.map(str::to_owned),
            entry: entry.map(str::to_owned),
        });
        let tree = match remote {
            Some(Some(Built::Tree { tree })) => tree,
            Some(_) => return None,
            None => self
                .with_plugin(erased.as_ref(), None, &params, |plugin, env| {
                    plugin.draw_settings(env)
                })
                .flatten()?,
        };
        let id = format!(
            "{}-settings-{}-{}",
            erased.name(),
            scope.unwrap_or_default(),
            entry.unwrap_or_default()
        );
        let origin = Origin { run: None, params };
        Some(self.plugin_tree(erased.name(), &id, &tree, origin, cx))
    }

    /// The title of `plugin`'s page `page`.
    pub(crate) fn plugin_page_title(
        &self,
        plugin: &str,
        page: &str,
        params: &BTreeMap<String, String>,
    ) -> Option<String> {
        let erased = registry().get(plugin)?;
        let remote = self.remote_view(View::Title {
            plugin: erased.name().to_owned(),
            page: page.to_owned(),
            params: params.clone(),
        });
        if let Some(remote) = remote {
            return match remote {
                Some(Built::Title { title }) => Some(title),
                _ => None,
            };
        }
        let run = params.get("run").and_then(|id| {
            self.runs.iter().find(|run| &*run.id.0 == id.as_str())
        });
        self.with_plugin(erased.as_ref(), run, params, |plugin, env| {
            plugin.page_title(page, env)
        })
        .flatten()
    }

    /// Each plugin's line in `run`'s plugin list: what a plugin with its
    /// UI says (`STATUS`), and the host's lines for the others.
    pub fn run_statuses(
        &self,
        run: &RunView,
    ) -> Vec<crate::view::PluginStatus> {
        let from = self
            .contributions(points::STATUS, &points::AtRun { run: run.info() });
        run.plugins
            .iter()
            .filter(|line| !from.iter().any(|status| status.name == line.name))
            .cloned()
            .chain(from.iter().cloned())
            .collect()
    }

    /// `run`'s plan: the host's fields, with what plugins set (`PLAN`)
    /// in place of a field of the same name.
    pub fn run_plan(&self, run: &RunView) -> Vec<crate::view::PlanField> {
        let from = self
            .contributions(points::PLAN, &points::AtRun { run: run.info() });
        let mut plan = run.plan.clone();
        for field in from {
            match plan.iter_mut().find(|own| own.name == field.name) {
                Some(own) => *own = field,
                None => plan.push(field),
            }
        }
        plan
    }

    /// Where the first context plugin steps in, as a share of `run`'s
    /// window.
    pub fn context_trigger(&self, run: &RunView) -> Option<f32> {
        let from = self.contributions(
            points::CONTEXT_TRIGGER,
            &points::AtRun { run: run.info() },
        );
        from.into_iter().min_by(|a, b| a.total_cmp(b))
    }

    /// What plugins add to `run`'s row in the sidebar.
    pub fn run_rows(&self, run: &RunView) -> Vec<tau_ui_plugin::RowNote> {
        self.contributions(points::RUN_ROW, &points::AtRun { run: run.info() })
    }

    /// The repository the composer is in: the open run's, else the one
    /// new runs start in.
    pub(crate) fn command_repo(&self) -> Option<String> {
        self.current()
            .filter(|_| self.route != Route::NewRun)
            .map(|run| run.repo.clone())
            .filter(|repo| !repo.is_empty())
            .or_else(|| self.selected_repo().map(str::to_owned))
    }

    /// `plugin`'s commands where the composer is: its own, and those it
    /// lists from its data and the composer's repository's.
    pub(crate) fn plugin_commands(
        &self,
        plugin: &dyn tau_ui_plugin::ErasedPlugin,
    ) -> Vec<tau_ui_plugin::registry::CommandInfo> {
        self.plugin_commands_in(plugin, self.command_repo().as_deref())
    }

    /// `plugin`'s commands where the composer is in `repo`, or in none.
    pub(crate) fn plugin_commands_in(
        &self,
        plugin: &dyn tau_ui_plugin::ErasedPlugin,
        repo: Option<&str>,
    ) -> Vec<tau_ui_plugin::registry::CommandInfo> {
        let name = plugin.name();
        let remote = self.remote_view(View::Commands {
            repo: repo.map(str::to_owned),
        });
        if let Some(remote) = remote {
            let Some(Built::Commands { commands }) = remote else {
                return Vec::new();
            };
            return commands
                .into_iter()
                .filter(|(by, _)| by == name)
                .map(|(_, command)| command)
                .collect();
        }
        let unset = PluginValue::default();
        let data = self.catalog.plugin_data.get(name).unwrap_or(&unset);
        let repo = repo.and_then(|repo| {
            let value = self.catalog.repo(repo)?.plugins.get(name)?;
            Some((repo, value))
        });
        plugin.commands(tau_ui_plugin::CommandsAt { data, repo })
    }

    /// Runs `text` as a plugin's slash command (`/name args`), on the run
    /// open, if a plugin has the command; says whether one did.
    pub(crate) fn run_plugin_command(
        &mut self,
        text: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(rest) = text.trim_start().strip_prefix('/') else {
            return false;
        };
        let (name, args) =
            rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let Some(plugin) = registry().plugins().find(|plugin| {
            self.plugin_commands(plugin.as_ref())
                .iter()
                .any(|command| command.name == name)
        }) else {
            return false;
        };
        let run = self
            .current()
            .filter(|_| self.route != Route::NewRun)
            .cloned();
        let repo = self.command_repo();
        if self.draws_remote_views() {
            let origin = Origin {
                run: run.map(|run| run.id),
                params: BTreeMap::new(),
            };
            let call = PluginCall::Command {
                name: name.to_owned(),
                args: args.trim().to_owned(),
                repo,
            };
            self.call_computer(plugin.name(), origin, call, cx);
            return true;
        }
        let params = BTreeMap::new();
        let ran = self
            .with_plugin(
                plugin.as_ref(),
                run.as_ref(),
                &params,
                |plugin, env| {
                    plugin.run_command(name, args.trim(), repo.as_deref(), env)
                },
            )
            .unwrap_or(false);
        self.drain_plugin_requests(cx);
        cx.notify();
        ran
    }

    /// Runs what `plugin`'s `key` does, in the repository the person is
    /// in: the one the sidebar has selected.
    pub(crate) fn press_plugin_key(
        &mut self,
        plugin: &str,
        key: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(erased) = registry().get(plugin) else {
            return;
        };
        let at = tau_ui_plugin::KeyAt {
            repo: self.selected_repo().map(str::to_owned),
        };
        if self.draws_remote_views() {
            let call = PluginCall::Key {
                key: key.to_owned(),
                repo: at.repo,
            };
            self.call_computer(erased.name(), Origin::default(), call, cx);
            return;
        }
        self.with_plugin(
            erased.as_ref(),
            None,
            &BTreeMap::new(),
            |plugin, env| plugin.press_key(key, &at, env),
        );
        self.drain_plugin_requests(cx);
        cx.notify();
    }

    /// The popover of `plugin`'s command `command` while it is written,
    /// with what follows its name.
    pub(crate) fn plugin_popover(
        &self,
        plugin: &str,
        command: &str,
        args: &str,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let erased = registry().get(plugin)?;
        let run = self.current().filter(|_| self.route != Route::NewRun);
        let remote = self.remote_view(View::Popover {
            plugin: erased.name().to_owned(),
            command: command.to_owned(),
            args: args.to_owned(),
            run: run.map(|run| run.id.clone()),
        });
        let tree = match remote {
            Some(Some(Built::Tree { tree })) => tree,
            Some(_) => return None,
            None => self
                .with_plugin(
                    erased.as_ref(),
                    run,
                    &BTreeMap::new(),
                    |plugin, env| plugin.command_popover(command, args, env),
                )
                .flatten()?,
        };
        let origin = Origin {
            run: run.map(|run| run.id.clone()),
            params: BTreeMap::new(),
        };
        let id = format!("{}-popover-{command}", erased.name());
        Some(self.plugin_tree(erased.name(), &id, &tree, origin, cx))
    }
}
