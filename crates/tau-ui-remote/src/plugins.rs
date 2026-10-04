//! The plugins tau-ui has, each with its UI (ADR 0017), and how the
//! workspace draws them: what they contribute at its points, their
//! pages, and what their handles ask.

use std::{collections::BTreeMap, rc::Rc, sync::LazyLock};

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
};

use crate::{
    view::RunView,
    workspace::{Workspace, WorkspaceEvent},
};

/// Every plugin, in the order the host adds them to a run's agent.
pub fn registry() -> &'static Registry {
    static REGISTRY: LazyLock<Registry> = LazyLock::new(|| {
        Registry::new()
            // The tools' cards: the host builds the tools themselves on
            // each run's workspace.
            .with(tau_tools::ui::ToolsUi)
            .with(tau_vcs::ui::VcsUi)
            .with(tau_reasoning::ReasoningPlugin)
            // Pruning first: it is cheaper than a summary, and summarizing
            // follows when pruning cannot help.
            .with(tau_fast_compaction::ui::FastCompactionUi)
            .with(tau_compaction::ui::CompactionUi)
            // The repository's rules check what the tools do; a goal's
            // hold of a stop comes after theirs.
            // Notes for what the tools do, and the repository's rules to
            // check it.
            .with(tau_memory::ui::MemoryUi)
            .with(tau_constitution::ui::ConstitutionUi)
            .with(tau_goal::GoalUi)
            // Questions the agent asks the person, answered in the
            // composer's place.
            .with(tau_ask::AskUi)
            // Agent commands in the repository's direnv environment, once
            // the person allows it.
            .with(tau_direnv::DirenvUi)
            // MCP servers' tools: their direct ones are added to the run's
            // plan in tau-mcp's `start`, so it comes before Codemode.
            .with(tau_mcp::McpUi)
            // Codemode's `start` lists the Luau signatures of the tools in
            // the run's plan when it runs. A plugin that adds tools in its
            // own `start` (tau-mcp, for its servers' direct tools) must be
            // registered before it, or scripts' signatures miss them.
            .with(tau_codemode::CodemodeUi)
    });
    &REGISTRY
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

/// A handle for `plugin` that asks `workspace`, through `queue`.
pub(crate) fn handle_for(
    plugin: &'static str,
    queue: Requests,
    workspace: gpui::WeakEntity<Workspace>,
) -> Handle {
    Handle::new(
        plugin,
        Rc::new(move |plugin, request, cx: &mut App| {
            queue.borrow_mut().push((plugin, request));
            let workspace = workspace.clone();
            cx.defer(move |cx| {
                let _ =
                    workspace.update(cx, |ws, cx| ws.drain_plugin_requests(cx));
            });
        }),
    )
}

impl Workspace {
    /// The handle `plugin`'s UI asks the workspace through.
    pub fn plugin_handle(&self, plugin: &'static str) -> Handle {
        handle_for(plugin, self.plugin_requests.clone(), self.weak.clone())
    }

    /// Carries out what plugins' handles asked.
    pub(crate) fn drain_plugin_requests(&mut self, cx: &mut Context<Self>) {
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
            }),
            Request::Navigate(link) => {
                let link = link.from(plugin);
                self.navigate(
                    crate::route::Route::Plugin {
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
            Request::Settings(settings) => {
                self.catalog.plugin_settings.insert(
                    plugin.to_owned(),
                    PluginValue::from_json(settings.clone()),
                );
                cx.emit(WorkspaceEvent::PluginSettings {
                    plugin: plugin.to_owned(),
                    settings,
                });
            }
            Request::Record { run, body } => {
                if let Some(view) =
                    self.runs.iter_mut().find(|view| view.id == run)
                {
                    view.fold(plugin, &body);
                }
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
            Request::Composer(text) => {
                self.composer
                    .update(cx, |input, cx| input.set_text(text, cx));
            }
            Request::RunDetails => {
                if self.compact() && !self.sheet_is_open() {
                    self.toggle_sheet(cx);
                }
            }
            Request::Submit => self.submit_from_button(cx),
            Request::Refresh => {}
            Request::OpenRun(run) => {
                self.navigate(crate::route::Route::Run(run), cx)
            }
            Request::AskJevKey => self.ask_for_jev_key_later(cx),
            Request::Focus(handle) => {
                self.plugin_focus = Some(handle);
                cx.notify();
            }
            Request::Cancel(run) => cx.emit(WorkspaceEvent::Cancel { run }),
        }
        cx.notify();
    }

    /// What a plugin's answer to an action says, for its UI.
    pub fn plugin_reply(
        &mut self,
        plugin: &str,
        reply: Value,
        cx: &mut Context<Self>,
    ) {
        if let (Some(erased), Some(ui)) =
            (registry().get(plugin), self.plugin_ui.get(plugin))
        {
            erased.reply(ui.clone(), reply, cx);
        }
        cx.notify();
    }

    /// Runs `f` with what `plugin` draws from: its values, the run when
    /// there is one, and the page's parameters.
    pub(crate) fn with_plugin<R>(
        &self,
        plugin: &dyn ErasedPlugin,
        run: Option<&RunView>,
        params: &BTreeMap<String, String>,
        cx: &mut App,
        f: impl FnOnce(&dyn ErasedPlugin, Env<'_>) -> R,
    ) -> Option<R> {
        let name = plugin.name();
        let ui = self.plugin_ui.get(name)?.clone();
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
        let settings = self
            .catalog
            .plugin_settings
            .get(name)
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
            ui,
            state: state.or(run.map(|_| &no_state)),
            data,
            settings,
            repos: &repos,
            run: info.as_ref(),
            params,
            compact: self.compact(),
            jev: self.catalog.models.access.jev,
            width: f32::from(self.screen_width()),
            handle: self.plugin_handle(name),
            runs: &runs,
            cards: &cards,
            cx,
        };
        Some(f(plugin, env))
    }

    /// What plugins contribute at `point`, in order: by each
    /// contribution's order, then the registry's.
    pub fn contributions<Cx: PointCx, Out: 'static>(
        &self,
        point: Point<Cx, Out>,
        at: &Cx,
        cx: &mut App,
    ) -> Vec<Out> {
        self.contributions_with(point, |_| Some(at), cx)
    }

    /// What `plugin` alone contributes at `point`: at one of its anchors.
    pub fn contributions_of<Cx: PointCx, Out: 'static>(
        &self,
        plugin: &str,
        point: Point<Cx, Out>,
        at: &Cx,
        cx: &mut App,
    ) -> Vec<Out> {
        self.contributions_with(
            point,
            |name| (name == plugin).then_some(at),
            cx,
        )
    }

    /// What plugins contribute at `point`, each with the context `at`
    /// gives it (none skips the plugin), in order.
    pub fn contributions_with<'c, Cx: PointCx, Out: 'static>(
        &self,
        point: Point<Cx, Out>,
        at: impl Fn(&str) -> Option<&'c Cx>,
        cx: &mut App,
    ) -> Vec<Out> {
        let empty = BTreeMap::new();
        let mut found: Vec<(i32, usize, Out)> = Vec::new();
        for (n, plugin) in registry().plugins().enumerate() {
            let Some(at) = at(plugin.name()) else {
                continue;
            };
            let run = at.run().and_then(|info| {
                self.runs.iter().find(|run| run.id == info.id)
            });
            let outs = self
                .with_plugin(plugin.as_ref(), run, &empty, cx, |plugin, env| {
                    plugin.contribute(point.name, at, env)
                })
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
                    found.push((order, n, *out));
                }
            }
        }
        found.sort_by_key(|(order, n, _)| (*order, *n));
        found.into_iter().map(|(_, _, out)| out).collect()
    }

    /// `plugin`'s page `page`, drawn with `params`.
    pub(crate) fn plugin_page(
        &self,
        plugin: &str,
        page: &str,
        params: &BTreeMap<String, String>,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let erased = registry().get(plugin)?;
        let run = params.get("run").and_then(|id| {
            self.runs.iter().find(|run| &*run.id.0 == id.as_str())
        });
        self.with_plugin(erased.as_ref(), run, params, cx, |plugin, env| {
            plugin.draw_page(page, env)
        })
        .flatten()
    }

    /// The title of `plugin`'s page `page`.
    pub(crate) fn plugin_page_title(
        &self,
        plugin: &str,
        page: &str,
        params: &BTreeMap<String, String>,
        cx: &mut App,
    ) -> Option<String> {
        let erased = registry().get(plugin)?;
        let run = params.get("run").and_then(|id| {
            self.runs.iter().find(|run| &*run.id.0 == id.as_str())
        });
        self.with_plugin(erased.as_ref(), run, params, cx, |plugin, env| {
            plugin.page_title(page, env)
        })
        .flatten()
    }

    /// Each plugin's line in `run`'s plugin list: what a plugin with its
    /// UI says (`STATUS`), and the host's lines for the others.
    pub fn run_statuses(
        &self,
        run: &RunView,
        cx: &mut App,
    ) -> Vec<crate::view::PluginStatus> {
        let from = self.contributions(
            points::STATUS,
            &points::AtRun { run: run.info() },
            cx,
        );
        run.plugins
            .iter()
            .filter(|line| !from.iter().any(|status| status.name == line.name))
            .cloned()
            .chain(from.iter().cloned())
            .collect()
    }

    /// `run`'s plan: the host's fields, with what plugins set (`PLAN`)
    /// in place of a field of the same name.
    pub fn run_plan(
        &self,
        run: &RunView,
        cx: &mut App,
    ) -> Vec<crate::view::PlanField> {
        let from = self.contributions(
            points::PLAN,
            &points::AtRun { run: run.info() },
            cx,
        );
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
    pub fn context_trigger(&self, run: &RunView, cx: &mut App) -> Option<f32> {
        let from = self.contributions(
            points::CONTEXT_TRIGGER,
            &points::AtRun { run: run.info() },
            cx,
        );
        from.into_iter().min_by(|a, b| a.total_cmp(b))
    }

    /// What plugins add to `run`'s row in the sidebar.
    pub fn run_rows(
        &self,
        run: &RunView,
        cx: &mut App,
    ) -> Vec<tau_ui_plugin::RowNote> {
        self.contributions(
            points::RUN_ROW,
            &points::AtRun { run: run.info() },
            cx,
        )
    }

    /// The repository the composer is in: the open run's, else the one
    /// new runs start in.
    pub(crate) fn command_repo(&self) -> Option<String> {
        self.current()
            .filter(|_| self.route != crate::route::Route::NewRun)
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
        let name = plugin.name();
        let unset = PluginValue::default();
        let data = self.catalog.plugin_data.get(name).unwrap_or(&unset);
        let repo = self.command_repo();
        let repo = repo.as_deref().and_then(|repo| {
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
            .filter(|_| self.route != crate::route::Route::NewRun)
            .cloned();
        let repo = self.command_repo();
        let params = BTreeMap::new();
        let ran = self
            .with_plugin(
                plugin.as_ref(),
                run.as_ref(),
                &params,
                cx,
                |plugin, env| {
                    plugin.run_command(name, args.trim(), repo.as_deref(), env)
                },
            )
            .unwrap_or(false);
        cx.notify();
        ran
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
        let run = self
            .current()
            .filter(|_| self.route != crate::route::Route::NewRun);
        self.with_plugin(
            erased.as_ref(),
            run,
            &BTreeMap::new(),
            cx,
            |plugin, env| plugin.command_popover(command, args, env),
        )
        .flatten()
    }
}
