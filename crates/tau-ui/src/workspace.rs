//! The window's root view: the runs, the screen on show and the way
//! back, and the layout for the window's width.
//!
//! The workspace never talks to an agent. Runs come in through
//! [`Workspace::apply_event`] and [`Workspace::update_run`], the rest of
//! what it shows through [`Catalog`]; what the user asks for goes out as a
//! [`WorkspaceEvent`], for the host to carry out.

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use gpui::{
    AnyElement,
    App,
    Context,
    Entity,
    EventEmitter,
    FocusHandle,
    Focusable,
    KeyBinding,
    ScrollHandle,
    ScrollWheelEvent,
    Subscription,
    Task,
    Window,
    actions,
    div,
    prelude::*,
    px,
};
use tau_agent::{event::RunEvent, tool::RunId};

use crate::{
    assets::Icon,
    catalog::{Catalog, PluginInfo, PluginScreen},
    input::{InputEvent, TextInput},
    route::{self, Route},
    theme::{NARROW_MAX, PHONE_MAX, Theme, theme},
    ui::{
        self,
        chrome,
        inspector::{self, Tab},
        screens,
        transcript,
    },
    view::{ChildKind, Proposal, RunStatus, RunUpdate, RunView, ToolState},
};

actions!(
    workspace,
    [
        GoBack,
        NewRun,
        ShowRuns,
        ShowMemory,
        ShowHistory,
        ShowPlugins
    ]
);

const CONTEXT: &str = "Workspace";

/// Binds the workspace's keys. [`crate::init`] calls it.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("escape", GoBack, Some(CONTEXT)),
        KeyBinding::new("alt-left", GoBack, Some(CONTEXT)),
        KeyBinding::new("ctrl-n", NewRun, Some(CONTEXT)),
        KeyBinding::new("ctrl-1", ShowRuns, Some(CONTEXT)),
        KeyBinding::new("ctrl-2", ShowMemory, Some(CONTEXT)),
        KeyBinding::new("ctrl-3", ShowHistory, Some(CONTEXT)),
        KeyBinding::new("ctrl-4", ShowPlugins, Some(CONTEXT)),
    ]);
}

/// What the user asked for. The host subscribes and acts on these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceEvent {
    /// Start a new run with this prompt.
    NewRun {
        prompt: String,
    },
    /// Queue a message for a running run (`Run::steer`).
    Steer {
        run: RunId,
        text: String,
    },
    Cancel {
        run: RunId,
    },
    /// Fork the run at its latest checkpoint.
    Fork {
        run: RunId,
    },
    /// Keep this branch of a fork and drop the others.
    KeepBranch {
        run: RunId,
    },
    /// Keep a note a memory plugin suggested.
    KeepNote {
        run: RunId,
        title: String,
    },
    /// Open a call a plugin flagged.
    ReviewCall {
        run: RunId,
        call_id: String,
    },
    /// Run a query against the store.
    Query {
        sql: String,
    },
}

pub struct Workspace {
    pub(crate) name: String,
    pub(crate) runs: Vec<RunView>,
    pub(crate) catalog: Catalog,
    pub(crate) route: Route,
    back_stack: Vec<Route>,
    /// The run the composer and the inspector act on.
    current: Option<RunId>,
    composer: Entity<TextInput>,
    pub(crate) history_filter: Entity<TextInput>,
    pub(crate) memory_search: Entity<TextInput>,
    tab: Tab,
    sheet_open: bool,
    /// Steering messages sent but not yet seen by the run, per run.
    queued: HashMap<RunId, String>,
    /// Notes kept, per run, by title.
    pub(crate) kept: HashMap<RunId, HashSet<String>>,
    /// Flagged calls someone looked at, as `(run, call id)`.
    pub(crate) dismissed: HashSet<(RunId, String)>,
    pub(crate) kept_branch: Option<RunId>,
    scroll: ScrollHandle,
    /// Keep the transcript at its bottom as the run grows. Scrolling up
    /// turns it off; scrolling back down turns it on.
    follow: bool,
    focus: FocusHandle,
    replay: Option<Task<()>>,
    /// Draw the phone layout in a phone-sized frame, whatever the width.
    phone_preview: bool,
    /// Lay out at exactly this size, pinned to the top left: for
    /// comparing screens against their designs.
    frame: Option<(f32, f32)>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Workspace {
    pub fn new(
        name: impl Into<String>,
        runs: Vec<RunView>,
        catalog: Catalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| TextInput::new("Start a new run", cx));
        let history_filter = cx.new(|cx| {
            TextInput::new("Filter by run, agent, model or stop", cx)
        });
        let memory_search = cx.new(|cx| TextInput::new("Search notes", cx));
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |ws, _, event: &InputEvent, _, cx| match event {
                    InputEvent::Submit(text) => ws.submit(text.clone(), cx),
                },
            ),
            // Filters apply as you type.
            cx.observe(&history_filter, |_, _, cx| cx.notify()),
            cx.observe(&memory_search, |_, _, cx| cx.notify()),
        ];
        composer.read(cx).focus_handle(cx).focus(window);
        let current = runs.first().map(|run| run.id.clone());
        let mut workspace = Self {
            name: name.into(),
            runs,
            catalog,
            route: Route::Home,
            back_stack: Vec::new(),
            current,
            composer,
            history_filter,
            memory_search,
            tab: Tab::Run,
            sheet_open: false,
            queued: HashMap::new(),
            kept: HashMap::new(),
            dismissed: HashSet::new(),
            kept_branch: None,
            scroll: ScrollHandle::new(),
            follow: true,
            focus: cx.focus_handle(),
            replay: None,
            phone_preview: false,
            frame: None,
            _subscriptions: subscriptions,
        };
        workspace.sync_placeholder(cx);
        workspace
    }

    pub fn runs(&self) -> &[RunView] {
        &self.runs
    }

    pub fn run(&self, id: &RunId) -> Option<&RunView> {
        self.runs.iter().find(|run| &run.id == id)
    }

    /// The run the composer and inspector act on.
    pub fn current(&self) -> Option<&RunView> {
        self.current.as_ref().and_then(|id| self.run(id))
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn set_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        self.catalog = catalog;
        cx.notify();
    }

    /// Adds a run at the top of the list and opens it.
    pub fn push_run(&mut self, run: RunView, cx: &mut Context<Self>) {
        let id = run.id.clone();
        self.runs.insert(0, run);
        self.navigate(Route::Run(id), cx);
    }

    /// Feeds a run event to every run it belongs to: its own run, and
    /// the parent that lists it as a child.
    pub fn apply_event(&mut self, event: &RunEvent, cx: &mut Context<Self>) {
        for run in &mut self.runs {
            run.apply(event);
        }
        if let RunEvent::TurnStart { run, .. } = event {
            // The run has read what was queued.
            self.queued.remove(run);
        }
        self.after_update(cx);
    }

    /// Applies an update to one run. Returns false if no run has that id.
    pub fn update_run(
        &mut self,
        run: &RunId,
        update: RunUpdate,
        cx: &mut Context<Self>,
    ) -> bool {
        if let RunUpdate::Event(event) = update {
            self.apply_event(&event, cx);
            return true;
        }
        let Some(view) = self.runs.iter_mut().find(|view| &view.id == run)
        else {
            return false;
        };
        view.update(update);
        self.after_update(cx);
        true
    }

    /// Plays scripted updates into a run, waiting before each one. For
    /// demos and for looking at the interface without an agent.
    pub fn replay(
        &mut self,
        run: RunId,
        steps: Vec<(Duration, RunUpdate)>,
        cx: &mut Context<Self>,
    ) {
        self.replay = Some(cx.spawn(async move |this, cx| {
            for (wait, update) in steps {
                if !wait.is_zero() {
                    cx.background_executor().timer(wait).await;
                }
                let applied = this.update(cx, |ws, cx| {
                    ws.update_run(&run, update, cx);
                });
                if applied.is_err() {
                    return;
                }
            }
        }));
    }

    /// Shows the phone layout in a 390×844 frame, for previewing it on a
    /// desktop.
    /// Lays the workspace out at `width`×`height` pixels in the window's
    /// top-left corner, whatever the window's size.
    pub fn set_frame(
        &mut self,
        frame: Option<(f32, f32)>,
        cx: &mut Context<Self>,
    ) {
        self.frame = frame;
        cx.notify();
    }

    pub fn set_phone_preview(&mut self, on: bool, cx: &mut Context<Self>) {
        self.phone_preview = on;
        cx.notify();
    }

    // Navigation.

    /// Opens a screen, remembering the current one for [`Self::back`].
    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if route == self.route {
            return;
        }
        let previous = std::mem::replace(&mut self.route, route);
        self.back_stack.push(previous);
        if self.back_stack.len() > 64 {
            self.back_stack.remove(0);
        }
        self.entered(cx);
    }

    /// Returns to the previous screen, or to the run list.
    pub fn back(&mut self, cx: &mut Context<Self>) {
        self.route = self.back_stack.pop().unwrap_or(Route::Home);
        self.entered(cx);
    }

    pub fn can_go_back(&self) -> bool {
        !self.back_stack.is_empty()
    }

    /// A phone tab starts its own history.
    pub fn switch_tab(&mut self, tab: route::Tab, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = tab.route();
        self.entered(cx);
    }

    fn entered(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.route.run().cloned() {
            if matches!(self.route, Route::Run(_))
                && self.current.as_ref() != Some(&run)
            {
                self.follow = true;
            }
            self.current = Some(run);
        }
        self.sheet_open = false;
        self.sync_placeholder(cx);
        cx.notify();
    }

    /// The screen that explains a plugin's work, for the current run.
    pub fn plugin_route(&self, plugin: &PluginInfo) -> Option<Route> {
        let run = self.current.clone();
        match plugin.screen? {
            PluginScreen::Plan => run.map(Route::Plan),
            PluginScreen::Memory => Some(Route::Memory { note: None }),
            PluginScreen::Constitution => {
                Some(Route::Constitution { rule: None })
            }
            PluginScreen::Ledger => {
                // The current run's ledger, or the latest run that has one.
                let has = |run: &RunView| run.last_rewrite().is_some();
                self.current()
                    .filter(|run| has(run))
                    .or_else(|| self.runs.iter().find(|run| has(run)))
                    .map(|run| Route::Ledger(run.id.clone()))
            }
        }
    }

    /// The screen for a plugin by name, about one run.
    pub fn plugin_route_named(&self, name: &str, run: &RunId) -> Option<Route> {
        let plugin = self.catalog.plugins.iter().find(|p| p.name == name)?;
        match plugin.screen? {
            PluginScreen::Plan => Some(Route::Plan(run.clone())),
            PluginScreen::Ledger => self
                .run(run)
                .filter(|run| run.last_rewrite().is_some())
                .map(|run| Route::Ledger(run.id.clone())),
            _ => self.plugin_route(plugin),
        }
    }

    /// Suggested notes nobody kept yet, with their run.
    pub fn pending_proposals(
        &self,
    ) -> impl Iterator<Item = (&RunId, &Proposal)> {
        self.runs.iter().flat_map(move |run| {
            run.proposals()
                .filter(move |proposal| {
                    !self
                        .kept
                        .get(&run.id)
                        .is_some_and(|kept| kept.contains(&proposal.title))
                })
                .map(move |proposal| (&run.id, proposal))
        })
    }

    // What the user asks for.

    /// Clears the composer and focuses it for a new prompt.
    pub fn start_new_run(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(Route::NewRun, cx);
        self.composer.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("Describe the task for a new run");
        });
        self.composer.read(cx).focus_handle(cx).focus(window);
    }

    pub fn keep_note(
        &mut self,
        run: &RunId,
        title: &str,
        cx: &mut Context<Self>,
    ) {
        let proposal = self.run(run).and_then(|view| {
            view.proposals().find(|p| p.title == title).cloned()
        });
        let from = self
            .run(run)
            .map_or(String::new(), |view| view.title.clone());
        if let Some(proposal) = proposal {
            self.catalog.memory.keep(&proposal, &from);
        }
        self.kept
            .entry(run.clone())
            .or_default()
            .insert(title.to_owned());
        cx.emit(WorkspaceEvent::KeepNote {
            run: run.clone(),
            title: title.to_owned(),
        });
        cx.notify();
    }

    /// Opens the rule that flagged a call, and tells the host.
    pub fn review_call(
        &mut self,
        run: &RunId,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        let rule = self.run(run).and_then(|view| view.tool(call_id)).and_then(
            |card| match &card.state {
                ToolState::Flagged { rule, .. }
                | ToolState::Blocked { rule, .. } => Some(rule.clone()),
                _ => None,
            },
        );
        cx.emit(WorkspaceEvent::ReviewCall {
            run: run.clone(),
            call_id: call_id.to_owned(),
        });
        self.navigate(Route::Constitution { rule }, cx);
    }

    pub fn keep_branch(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.kept_branch = Some(run.clone());
        cx.emit(WorkspaceEvent::KeepBranch { run: run.clone() });
        cx.notify();
    }

    pub fn run_query(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::Query {
            sql: self.catalog.store.sample_query.clone(),
        });
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.current() {
            cx.emit(WorkspaceEvent::Cancel {
                run: run.id.clone(),
            });
        }
    }

    fn fork(&mut self, cx: &mut Context<Self>) {
        if let Some(run) = self.current() {
            cx.emit(WorkspaceEvent::Fork {
                run: run.id.clone(),
            });
        }
    }

    pub fn toggle_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet_open = !self.sheet_open;
        cx.notify();
    }

    pub fn set_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.tab = tab;
        cx.notify();
    }

    /// Submits `text` as if typed into the composer: it steers the open
    /// run if one is live, or starts a new run.
    pub fn submit_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        self.submit(text, cx);
    }

    fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        match self.current().filter(|_| self.route != Route::NewRun) {
            Some(run) if run.status.is_live() => {
                let run = run.id.clone();
                self.queued.insert(run.clone(), text.clone());
                cx.emit(WorkspaceEvent::Steer { run, text });
            }
            _ => cx.emit(WorkspaceEvent::NewRun { prompt: text }),
        }
        cx.notify();
    }

    fn submit_from_button(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().trim().to_owned();
        if !text.is_empty() {
            self.composer.update(cx, |input, cx| input.clear(cx));
            self.submit(text, cx);
        }
    }

    fn after_update(&mut self, cx: &mut Context<Self>) {
        self.sync_placeholder(cx);
        cx.notify();
    }

    fn sync_placeholder(&mut self, cx: &mut Context<Self>) {
        let live = self.route != Route::NewRun && self.is_live();
        self.composer.update(cx, |input, _| {
            input.set_placeholder(if live {
                "Steer the run, or @ a file, agent or checkpoint"
            } else {
                "Start a new run"
            })
        });
    }

    fn is_live(&self) -> bool {
        self.current().is_some_and(|run| run.status.is_live())
    }

    // Layouts.

    fn transcript(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let run = self.current().filter(|_| self.route != Route::NewRun);
        let items = match run {
            Some(run) => transcript::items(self, run, t, compact, cx),
            None => vec![
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(12.))
                    .pt(px(120.))
                    .text_color(t.muted)
                    .child(chrome::logo(t, 40.))
                    .child("A new run. Describe the task below.")
                    .into_any_element(),
            ],
        };
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        div()
            .id("transcript")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_scroll_wheel(cx.listener(
                |ws, event: &ScrollWheelEvent, _, _| {
                    let up = event.delta.pixel_delta(px(20.)).y > px(0.);
                    let bottom = -ws.scroll.max_offset().height;
                    ws.follow = !up && ws.scroll.offset().y <= bottom + px(40.);
                },
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .px(px(if compact { 16. } else { 24. }))
                    .py(px(if compact { 16. } else { 20. }))
                    .children(items),
            )
    }

    fn run_header(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let Some(run) = self.current().filter(|_| self.route != Route::NewRun)
        else {
            return div()
                .h(px(48.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(24.))
                .border_b_1()
                .border_color(t.border)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child("New run");
        };
        let (color, label) = ui::status_look(&run.status, t);
        let live = run.status.is_live();
        div()
            .h(px(48.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(24.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(run.title.clone()),
            )
            .child(ui::pill(
                match run.status {
                    RunStatus::Running => {
                        format!("{label} · turn {}", run.turn)
                    }
                    _ => label.to_string(),
                },
                color,
                t.raised,
            ))
            .child(ui::mono(
                format!("{} · turn {}", run.agent, run.turn),
                12.,
                t.dim,
            ))
            .child(div().flex_1())
            .child(ui::mono(crate::view::usd(run.usage.cost), 12., t.muted))
            .children(
                run.children
                    .iter()
                    .find(|child| child.kind == ChildKind::Fork)
                    .map(|fork| {
                        let route = Route::Compare {
                            main: run.id.clone(),
                            fork: fork.id.clone(),
                        };
                        div()
                            .id("compare")
                            .child(ui::button("Compare forks", t))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.navigate(route.clone(), cx)
                            }))
                    }),
            )
            .child(
                div()
                    .id("fork")
                    .child(ui::button("Fork here", t).child(ui::icon(
                        Icon::Fork,
                        13.,
                        t.text_soft,
                    )))
                    .on_click(cx.listener(|ws, _, _, cx| ws.fork(cx))),
            )
            .when(live, |header| {
                header.child(
                    div()
                        .id("cancel")
                        .child(
                            ui::button("Cancel", t)
                                .border_color(t.red_border)
                                .text_color(t.red),
                        )
                        .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                )
            })
    }

    fn composer(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let live = self.is_live();
        let queued = self
            .current()
            .and_then(|run| self.queued.get(&run.id))
            .cloned();
        let send = div()
            .id("send")
            .on_click(cx.listener(|ws, _, _, cx| ws.submit_from_button(cx)));
        let send = if compact {
            send.size(px(44.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(22.))
                .bg(t.accent)
                .cursor_pointer()
                .child(ui::icon(Icon::Send, 18., t.bg))
        } else {
            send.child(ui::primary_button(
                if live { "Steer" } else { "Start" },
                t,
            ))
        };

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(if compact { 12. } else { 24. }))
            .pt(px(if compact { 10. } else { 0. }))
            .pb(px(if compact { 18. } else { 16. }))
            .when(compact, |bar| bar.border_t_1().border_color(t.border))
            .when_some(queued, |bar, text| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(12.))
                        .py(px(7.))
                        .rounded(px(6.))
                        .bg(t.blue_soft)
                        .border_1()
                        .border_color(t.blue_border)
                        .text_size(px(12.))
                        .child(ui::icon(Icon::Chevron, 13., t.blue))
                        .child(div().text_color(t.blue).child("Steer queued"))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(t.text_soft)
                                .child(format!(
                                    "\u{201c}{text}\u{201d}. Lands after this tool batch."
                                )),
                        ),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .when(compact && live, |row| {
                        row.child(
                            div()
                                .id("phone-cancel")
                                .size(px(44.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(22.))
                                .border_1()
                                .border_color(t.red_border)
                                .cursor_pointer()
                                .child(ui::icon(Icon::Stop, 14., t.red))
                                .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .min_h(px(44.))
                            .px(px(14.))
                            .border_1()
                            .border_color(t.border_strong)
                            .rounded(px(if compact { 22. } else { 10. }))
                            .bg(t.panel)
                            .when(!compact, |field| {
                                field.child(ui::icon(Icon::Paperclip, 14., t.muted))
                            })
                            .child(self.composer.clone())
                            .when(!compact, |field| {
                                field.child(ui::mono(
                                    if live { "Enter steers" } else { "Enter starts" },
                                    11.,
                                    t.dim,
                                ))
                            }),
                    )
                    .child(send),
            )
    }

    fn inspector(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let tabs = Tab::ALL.into_iter().map(|tab| {
            div()
                .id(tab.label())
                .child(inspector::tab_label(tab, tab == self.tab, t))
                .on_click(cx.listener(move |ws, _, _, cx| ws.set_tab(tab, cx)))
        });
        div()
            .flex_1()
            .flex()
            .flex_col()
            .min_h(px(0.))
            .bg(t.panel)
            .border_l_1()
            .border_color(t.border)
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .flex()
                    .items_end()
                    .gap(px(18.))
                    .px(px(16.))
                    .border_b_1()
                    .border_color(t.border)
                    .children(tabs),
            )
            .child(
                div()
                    .id("inspector")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p(px(16.))
                    .children(self.current().map(|run| {
                        inspector::content(self, run, self.tab, t, cx)
                    })),
            )
    }

    /// The run screen: its header, transcript and composer.
    fn run_screen(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        div()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .when(!compact, |screen| screen.child(self.run_header(t, cx)))
            .child(self.transcript(compact, t, cx))
            .child(self.composer(compact, t, cx))
    }

    /// Any screen but a run's.
    fn screen(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.route {
            Route::Home | Route::Run(_) | Route::NewRun => {
                self.run_screen(compact, t, cx).into_any_element()
            }
            Route::History => screens::history::render(self, compact, t, cx),
            Route::Compare { main, fork } => {
                screens::compare::render(self, main, fork, compact, t, cx)
            }
            Route::Plugins => screens::plugins::render(self, compact, t, cx),
            Route::Plan(run) => {
                screens::plan::render(self, run, compact, t, cx)
            }
            Route::Memory { note } => {
                screens::memory::render(self, note.as_deref(), compact, t, cx)
            }
            Route::Constitution { rule } => screens::constitution::render(
                self,
                rule.as_deref(),
                compact,
                t,
                cx,
            ),
            Route::Ledger(run) => {
                screens::ledger::render(self, run, compact, t, cx)
            }
        }
    }

    fn desktop(
        &self,
        wide: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let on_run = matches!(self.route, Route::Home | Route::Run(_));
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(chrome::title_bar(self, t, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .flex()
                    .child(
                        chrome::sidebar(self, t, cx)
                            .w(px(if wide { 264. } else { 232. }))
                            .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(self.screen(false, t, cx)),
                    )
                    .when(wide && on_run, |row| {
                        row.child(
                            div()
                                .w(px(328.))
                                .flex_shrink_0()
                                .flex()
                                .flex_col()
                                .child(self.inspector(t, cx)),
                        )
                    }),
            )
            .child(chrome::status_bar(self, t))
            .into_any_element()
    }

    fn phone(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let screen = div().size_full().relative().flex().flex_col();
        match (&self.route, self.current()) {
            (Route::Run(_), Some(run)) => screen
                .child(chrome::phone_run_bar(run, t, cx))
                .child(self.run_screen(true, t, cx))
                .when(self.sheet_open, |screen| {
                    screen.child(self.sheet(run, t, cx))
                })
                .into_any_element(),
            (Route::Home, _) => screen
                .child(chrome::phone_header(self, t, cx))
                .child(chrome::phone_run_list(self, t, cx))
                .child(chrome::phone_tab_bar(self, t, cx))
                .into_any_element(),
            _ => screen
                .child(chrome::phone_header(self, t, cx))
                .child(
                    div()
                        .flex_1()
                        .min_h(px(0.))
                        .flex()
                        .flex_col()
                        .child(self.screen(true, t, cx)),
                )
                .when(self.route.is_top_level(), |screen| {
                    screen.child(chrome::phone_tab_bar(self, t, cx))
                })
                .into_any_element(),
        }
    }

    fn sheet(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let segments = Tab::ALL.into_iter().map(|tab| {
            let active = tab == self.tab;
            div()
                .id(("sheet-tab", tab as usize))
                .h(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .text_size(px(13.))
                .cursor_pointer()
                .text_color(if active { t.text } else { t.muted })
                .when(active, |segment| segment.bg(t.selected))
                .child(tab.label())
                .on_click(cx.listener(move |ws, _, _, cx| ws.set_tab(tab, cx)))
        });
        let live = run.status.is_live();
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .child(
                div()
                    .id("scrim")
                    .flex_1()
                    .bg(t.scrim)
                    .on_click(cx.listener(|ws, _, _, cx| ws.toggle_sheet(cx))),
            )
            .child(
                div()
                    .h(gpui::relative(0.72))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .px(px(16.))
                    .pt(px(8.))
                    .pb(px(24.))
                    .bg(t.panel)
                    .border_t_1()
                    .border_color(t.border_strong)
                    .rounded_t(px(18.))
                    .child(
                        div().flex().justify_center().child(
                            div()
                                .w(px(40.))
                                .h(px(4.))
                                .rounded(px(2.))
                                .bg(t.border_strong),
                        ),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(Tab::ALL.len() as u16)
                            .gap(px(4.))
                            .p(px(4.))
                            .rounded(px(10.))
                            .bg(t.bg)
                            .children(segments),
                    )
                    .child(
                        div()
                            .id("sheet-body")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .child(inspector::content(
                                self, run, self.tab, t, cx,
                            )),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(if live { 2 } else { 1 })
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("sheet-fork")
                                    .child(
                                        ui::button("Fork here", t)
                                            .h(px(44.))
                                            .rounded(px(10.)),
                                    )
                                    .on_click(
                                        cx.listener(|ws, _, _, cx| ws.fork(cx)),
                                    ),
                            )
                            .when(live, |row| {
                                row.child(
                                    div()
                                        .id("sheet-cancel")
                                        .child(
                                            ui::button("Cancel run", t)
                                                .h(px(44.))
                                                .rounded(px(10.))
                                                .border_color(t.red_border)
                                                .text_color(t.red),
                                        )
                                        .on_click(cx.listener(
                                            |ws, _, _, cx| ws.cancel(cx),
                                        )),
                                )
                            }),
                    ),
            )
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = theme(cx).clone();
        let width = match self.frame {
            Some((width, _)) => px(width),
            None => window.viewport_size().width,
        };
        let phone = self.phone_preview || width < PHONE_MAX;
        let body = if phone {
            self.phone(&t, cx)
        } else {
            self.desktop(width >= NARROW_MAX, &t, cx)
        };
        let body = if self.phone_preview {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(gpui::rgb(0x0b0c0e))
                .child(
                    div()
                        .w(px(390.))
                        .h(px(844.))
                        .max_h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(px(28.))
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.bg)
                        .child(body),
                )
                .into_any_element()
        } else if let Some((width, height)) = self.frame {
            div()
                .size_full()
                .bg(gpui::rgb(0x0b0c0e))
                .child(
                    div()
                        .w(px(width))
                        .h(px(height))
                        .overflow_hidden()
                        .bg(t.bg)
                        .child(body),
                )
                .into_any_element()
        } else {
            body
        };
        div()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|ws, _: &GoBack, _, cx| ws.back(cx)))
            .on_action(cx.listener(|ws, _: &NewRun, window, cx| {
                ws.start_new_run(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowRuns, _, cx| {
                ws.switch_tab(route::Tab::Runs, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowMemory, _, cx| {
                ws.switch_tab(route::Tab::Memory, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowHistory, _, cx| {
                ws.switch_tab(route::Tab::History, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowPlugins, _, cx| {
                ws.switch_tab(route::Tab::Plugins, cx)
            }))
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(crate::theme::SANS)
            .text_size(px(if phone { 14.5 } else { 13. }))
            .child(body)
    }
}
