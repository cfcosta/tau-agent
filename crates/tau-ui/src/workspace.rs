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
use tau_agent::{
    event::{RunEvent, StopReason},
    tool::RunId,
};

use crate::{
    assets::Icon,
    catalog::{Catalog, PluginInfo, PluginScreen},
    input::{InputEvent, TextInput},
    pull_request::{PrState, PullRequest},
    route::{self, Route},
    setup::{
        CloneState,
        GitHub,
        ModelAccess,
        RepoClone,
        Setup,
        SetupStep,
        SetupUpdate,
    },
    theme::{
        Design as _,
        IconSize,
        NARROW_MAX,
        PHONE_MAX,
        Theme,
        Type,
        radius,
        sp,
        theme,
        weight,
    },
    ui::{
        self,
        chrome,
        components::ButtonKind,
        inspector::{self, Tab},
        screens,
        transcript,
    },
    view::{
        ChildKind,
        ChildRun,
        CodeState,
        Origin,
        Proposal,
        RunStatus,
        RunUpdate,
        RunView,
        ToolState,
    },
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
    /// Fork the run after `turn` (its latest turn when `None`): a new
    /// run on `prompt`, from that turn's conversation and code.
    Fork {
        run: RunId,
        turn: Option<u32>,
        prompt: String,
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
    /// Start GitHub's device sign-in; answer with a
    /// [`GitHub::Waiting`] code, then [`GitHub::SignedIn`].
    GitHubSignIn,
    /// The user says they approved the sign-in: check now rather than at
    /// the next poll.
    GitHubCheck,
    /// Check a fine-grained personal access token.
    GitHubToken {
        token: String,
    },
    /// Sign in to ChatGPT for Codex, in the browser or with a device
    /// code.
    CodexSignIn {
        device: bool,
    },
    /// Use an OpenAI API key.
    ApiKey {
        key: String,
    },
    /// Clone these repositories (`owner/name`) into tau's storage.
    CloneRepos {
        repos: Vec<String>,
    },
    /// Write a pull request draft from the run, for
    /// [`Workspace::set_pull_request`].
    PreparePullRequest {
        run: RunId,
    },
    /// Diff the code of a run and its fork, for
    /// [`Workspace::set_branch_code`].
    CompareCode {
        main: RunId,
        fork: RunId,
    },
    /// Push the run's branch and open the pull request.
    CreatePullRequest {
        run: RunId,
        title: String,
        body: String,
        draft: bool,
        keep_pushing: bool,
        reviewers: Vec<String>,
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
    /// The composer is writing a fork of this run, after this turn.
    forking: Option<(RunId, u32)>,
    /// Something went wrong that the user must read: a title and why.
    alert: Option<(String, String)>,
    /// Notes kept, per run, by title.
    pub(crate) kept: HashMap<RunId, HashSet<String>>,
    /// Flagged calls someone looked at, as `(run, call id)`.
    pub(crate) dismissed: HashSet<(RunId, String)>,
    pub(crate) kept_branch: Option<RunId>,
    pub(crate) setup: Setup,
    pub(crate) pull_requests: HashMap<RunId, PullRequest>,
    /// The code of each comparison opened, by (run, fork).
    pub(crate) branch_code: HashMap<(RunId, RunId), CodeState>,
    pub(crate) github_token: Entity<TextInput>,
    pub(crate) api_key: Entity<TextInput>,
    pub(crate) repo_filter: Entity<TextInput>,
    pub(crate) first_task: Entity<TextInput>,
    pub(crate) pr_title: Entity<TextInput>,
    pub(crate) reviewers: Entity<TextInput>,
    scroll: ScrollHandle,
    /// Keep the transcript at its bottom as the run grows. Scrolling up
    /// turns it off; scrolling back down turns it on.
    follow: bool,
    focus: FocusHandle,
    /// Scripted runs playing, for demos. Several can play at once.
    replays: Vec<Task<()>>,
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
        let github_token =
            cx.new(|cx| TextInput::new("github_pat_…", cx).masked());
        let api_key = cx.new(|cx| TextInput::new("sk-…", cx).masked());
        let repo_filter =
            cx.new(|cx| TextInput::new("Filter your repositories", cx));
        let first_task = cx.new(|cx| {
            TextInput::new(
                "Describe the task, for example: the retry loop ignores \
                 retry-after on 429s. Honor it and add tests.",
                cx,
            )
        });
        let pr_title =
            cx.new(|cx| TextInput::new("Title", cx).keep_on_submit());
        let reviewers =
            cx.new(|cx| TextInput::new("@reviewer", cx).keep_on_submit());
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
            cx.observe(&repo_filter, |_, _, cx| cx.notify()),
            cx.subscribe(&github_token, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(token) = event;
                ws.submit_token(token.clone(), cx);
            }),
            cx.subscribe(&api_key, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(key) = event;
                ws.submit_api_key(key.clone(), cx);
            }),
            cx.subscribe(&first_task, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(task) = event;
                ws.start_first_run(task.clone(), cx);
            }),
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
            forking: None,
            alert: None,
            kept: HashMap::new(),
            dismissed: HashSet::new(),
            kept_branch: None,
            setup: Setup::default(),
            pull_requests: HashMap::new(),
            branch_code: HashMap::new(),
            github_token,
            api_key,
            repo_filter,
            first_task,
            pr_title,
            reviewers,
            scroll: ScrollHandle::new(),
            follow: true,
            focus: cx.focus_handle(),
            replays: Vec::new(),
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

    /// Adds a run at the top of the list and opens it. A fork is listed
    /// under the run it came from, too.
    pub fn push_run(&mut self, run: RunView, cx: &mut Context<Self>) {
        let id = run.id.clone();
        if let Origin::Fork { from, .. } = &run.origin
            && let Some(parent) =
                self.runs.iter_mut().find(|view| &view.id == from)
        {
            parent.children.push(ChildRun {
                id: id.clone(),
                title: run.title.clone(),
                kind: ChildKind::Fork,
                status: run.status.clone(),
            });
        }
        self.runs.insert(0, run);
        self.navigate(Route::Run(id), cx);
    }

    /// Adds runs from earlier sessions below the ones already shown.
    pub fn add_history(&mut self, runs: Vec<RunView>, cx: &mut Context<Self>) {
        for run in runs {
            if self.run(&run.id).is_none() {
                self.runs.push(run);
            }
        }
        if self.current.is_none() {
            self.current = self.runs.first().map(|run| run.id.clone());
        }
        cx.notify();
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
        self.replays.push(cx.spawn(async move |this, cx| {
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
        // Comparing two branches shows their code; ask for it once.
        if let Route::Compare { main, fork } = &self.route {
            let key = (main.clone(), fork.clone());
            if let std::collections::hash_map::Entry::Vacant(entry) =
                self.branch_code.entry(key)
            {
                entry.insert(CodeState::Loading);
                cx.emit(WorkspaceEvent::CompareCode {
                    main: main.clone(),
                    fork: fork.clone(),
                });
            }
        }
        // A fork being written belongs to the run it forks.
        if self
            .forking
            .as_ref()
            .is_some_and(|(run, _)| self.route.run() != Some(run))
        {
            self.forking = None;
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

    /// The last turn of `run` that can be forked from: its last finished
    /// turn.
    fn last_fork_turn(run: &RunView) -> u32 {
        if run.status.is_live() {
            run.turn.saturating_sub(1)
        } else {
            run.turn
        }
    }

    /// Whether a fork can start after `turn` of `run`: the turn has
    /// ended.
    pub fn can_fork_at(run: &RunView, turn: u32) -> bool {
        turn >= 1 && turn <= Self::last_fork_turn(run)
    }

    /// Puts the composer in fork mode: the next message starts a fork of
    /// the current run, after its latest finished turn.
    pub fn start_fork(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(run) = self.current() else { return };
        let (id, turn) = (run.id.clone(), Self::last_fork_turn(run).max(1));
        self.fork_from(&id, turn, window, cx);
    }

    /// Puts the composer in fork mode for a fork of `run` after `turn`,
    /// as the transcript's "Fork from here" does.
    pub fn fork_from(
        &mut self,
        run: &RunId,
        turn: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(run) = self.run(run) else { return };
        let (id, turn) = (
            run.id.clone(),
            turn.clamp(1, Self::last_fork_turn(run).max(1)),
        );
        if !matches!(self.route, Route::Run(_) | Route::Home) {
            self.navigate(Route::Run(id.clone()), cx);
        }
        self.forking = Some((id, turn));
        self.sheet_open = false;
        self.composer.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("What should the fork try instead?");
        });
        self.composer.read(cx).focus_handle(cx).focus(window);
        cx.notify();
    }

    /// Moves the fork point one turn back or forward.
    fn step_fork(&mut self, delta: i32, cx: &mut Context<Self>) {
        let Some((run, turn)) = self.forking.clone() else {
            return;
        };
        let last = self.run(&run).map_or(1, Self::last_fork_turn).max(1);
        let turn = turn.saturating_add_signed(delta).clamp(1, last);
        self.forking = Some((run, turn));
        cx.notify();
    }

    fn cancel_fork(&mut self, cx: &mut Context<Self>) {
        self.forking = None;
        self.sync_placeholder(cx);
        cx.notify();
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
        if let Some((run, turn)) = self.forking.take() {
            cx.emit(WorkspaceEvent::Fork {
                run,
                turn: Some(turn),
                prompt: text,
            });
            self.sync_placeholder(cx);
            cx.notify();
            return;
        }
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
        let forking = self.forking.is_some();
        self.composer.update(cx, |input, _| {
            input.set_placeholder(if forking {
                "What should the fork try instead?"
            } else if live {
                "Steer the run, or @ a file, agent or checkpoint"
            } else {
                "Start a new run"
            })
        });
    }

    fn is_live(&self) -> bool {
        self.current().is_some_and(|run| run.status.is_live())
    }

    // Onboarding.

    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    /// Replaces what onboarding knows, as when resuming it.
    pub fn set_setup(&mut self, setup: Setup, cx: &mut Context<Self>) {
        self.setup = setup;
        cx.notify();
    }

    /// Opens onboarding at `step`, with no way back to the runs until it
    /// is done.
    pub fn start_setup(&mut self, step: SetupStep, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = Route::Setup(step);
        self.entered(cx);
    }

    /// Records what the host learned, and moves on when a step is done.
    pub fn update_setup(
        &mut self,
        update: SetupUpdate,
        cx: &mut Context<Self>,
    ) {
        let signed_in =
            matches!(update, SetupUpdate::GitHub(GitHub::SignedIn { .. }));
        let connected =
            matches!(update, SetupUpdate::Model(ModelAccess::Connected { .. }));
        self.setup.update(update);
        match self.route {
            Route::Setup(SetupStep::GitHub | SetupStep::Token) if signed_in => {
                self.navigate(Route::Setup(SetupStep::Model), cx)
            }
            Route::Setup(SetupStep::Model) if connected => {
                if self.setup.repos.is_empty() {
                    self.finish_setup(cx);
                } else {
                    self.navigate(Route::Setup(SetupStep::Repos), cx);
                }
            }
            _ => cx.notify(),
        }
    }

    /// Leaves onboarding for a new run.
    pub fn finish_setup(&mut self, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = if self.runs.is_empty() {
            Route::NewRun
        } else {
            Route::Home
        };
        self.entered(cx);
    }

    pub fn sign_in_github(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.setup.github, GitHub::Waiting(_)) {
            self.setup.github = GitHub::SignedOut;
            cx.emit(WorkspaceEvent::GitHubSignIn);
        }
        self.navigate(Route::Setup(SetupStep::GitHub), cx);
    }

    pub(crate) fn submit_token(
        &mut self,
        token: String,
        cx: &mut Context<Self>,
    ) {
        let token = token.trim().to_owned();
        if token.is_empty() {
            return;
        }
        self.github_token.update(cx, |input, cx| input.clear(cx));
        self.setup.github = GitHub::Checking;
        cx.emit(WorkspaceEvent::GitHubToken { token });
        cx.notify();
    }

    pub(crate) fn submit_token_from_button(&mut self, cx: &mut Context<Self>) {
        let token = self.github_token.read(cx).text().to_owned();
        self.submit_token(token, cx);
    }

    pub fn sign_in_codex(&mut self, device: bool, cx: &mut Context<Self>) {
        self.setup.model = ModelAccess::SigningIn {
            url: None,
            device: None,
        };
        cx.emit(WorkspaceEvent::CodexSignIn { device });
        cx.notify();
    }

    pub(crate) fn submit_api_key(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let key = key.trim().to_owned();
        if key.is_empty() {
            return;
        }
        self.api_key.update(cx, |input, cx| input.clear(cx));
        self.setup.model = ModelAccess::SigningIn {
            url: None,
            device: None,
        };
        cx.emit(WorkspaceEvent::ApiKey { key });
        cx.notify();
    }

    pub(crate) fn submit_api_key_from_button(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let key = self.api_key.read(cx).text().to_owned();
        self.submit_api_key(key, cx);
    }

    pub fn toggle_repo(&mut self, name: &str, cx: &mut Context<Self>) {
        self.setup.toggle(name);
        cx.notify();
    }

    /// Clones the picked repositories and opens the first run's screen.
    pub fn clone_selected(&mut self, cx: &mut Context<Self>) {
        let repos: Vec<String> = self
            .setup
            .selected()
            .map(|repo| repo.name.clone())
            .collect();
        for name in &repos {
            if !self.setup.clones.iter().any(|clone| &clone.name == name) {
                self.setup.clones.push(RepoClone {
                    name: name.clone(),
                    state: CloneState::Cloning {
                        share: 0.0,
                        detail: "starting".into(),
                    },
                });
            }
        }
        if !repos.is_empty() {
            cx.emit(WorkspaceEvent::CloneRepos { repos });
        }
        self.navigate(Route::Setup(SetupStep::Ready), cx);
    }

    pub(crate) fn start_first_run(
        &mut self,
        task: String,
        cx: &mut Context<Self>,
    ) {
        let task = task.trim().to_owned();
        if task.is_empty() {
            return;
        }
        self.first_task.update(cx, |input, cx| input.clear(cx));
        self.finish_setup(cx);
        cx.emit(WorkspaceEvent::NewRun { prompt: task });
    }

    pub(crate) fn start_first_run_from_button(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let task = self.first_task.read(cx).text().to_owned();
        self.start_first_run(task, cx);
    }

    /// Shows a dialog: something failed that the user asked for.
    pub fn show_alert(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.alert = Some((title.into(), message.into()));
        cx.notify();
    }

    pub fn alert(&self) -> Option<(&str, &str)> {
        self.alert
            .as_ref()
            .map(|(title, message)| (title.as_str(), message.as_str()))
    }

    pub fn dismiss_alert(&mut self, cx: &mut Context<Self>) {
        self.alert = None;
        cx.notify();
    }

    /// Esc: closes a dialog if one is open, else goes back.
    pub fn escape(&mut self, cx: &mut Context<Self>) {
        if self.alert.is_some() {
            self.dismiss_alert(cx);
        } else {
            self.back(cx);
        }
    }

    /// The code of a comparison, as the host found it.
    pub fn set_branch_code(
        &mut self,
        main: &RunId,
        fork: &RunId,
        code: CodeState,
        cx: &mut Context<Self>,
    ) {
        self.branch_code.insert((main.clone(), fork.clone()), code);
        cx.notify();
    }

    pub fn branch_code(
        &self,
        main: &RunId,
        fork: &RunId,
    ) -> Option<&CodeState> {
        self.branch_code.get(&(main.clone(), fork.clone()))
    }

    // Pull requests.

    pub fn pull_request(&self, run: &RunId) -> Option<&PullRequest> {
        self.pull_requests.get(run)
    }

    /// Opens the pull request screen for a run, asking the host for a
    /// draft if there is none yet.
    pub fn open_pull_request(&mut self, run: &RunId, cx: &mut Context<Self>) {
        match self.pull_requests.get(run) {
            Some(pr) => {
                let title = pr.title.clone();
                self.pr_title
                    .update(cx, |input, cx| input.set_text(title, cx));
            }
            None => {
                cx.emit(WorkspaceEvent::PreparePullRequest { run: run.clone() })
            }
        }
        self.navigate(Route::PullRequest(run.clone()), cx);
    }

    /// The draft the host wrote from a run.
    pub fn set_pull_request(
        &mut self,
        run: &RunId,
        pr: PullRequest,
        cx: &mut Context<Self>,
    ) {
        if self.route == Route::PullRequest(run.clone()) {
            let title = pr.title.clone();
            self.pr_title
                .update(cx, |input, cx| input.set_text(title, cx));
        }
        self.pull_requests.insert(run.clone(), pr);
        cx.notify();
    }

    /// What became of a pull request: creating, opened or failed.
    pub fn set_pull_request_state(
        &mut self,
        run: &RunId,
        state: PrState,
        cx: &mut Context<Self>,
    ) {
        if let Some(pr) = self.pull_requests.get_mut(run) {
            pr.state = state;
            cx.notify();
        }
    }

    pub(crate) fn toggle_pr_option(
        &mut self,
        run: &RunId,
        draft: bool,
        cx: &mut Context<Self>,
    ) {
        if let Some(pr) = self.pull_requests.get_mut(run) {
            if draft {
                pr.draft = !pr.draft;
            } else {
                pr.keep_pushing = !pr.keep_pushing;
            }
            cx.notify();
        }
    }

    pub fn create_pull_request(&mut self, run: &RunId, cx: &mut Context<Self>) {
        let title = self.pr_title.read(cx).text().trim().to_owned();
        let reviewers: Vec<String> = self
            .reviewers
            .read(cx)
            .text()
            .split([',', ' '])
            .map(|name| name.trim().trim_start_matches('@'))
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        let Some(pr) = self.pull_requests.get_mut(run) else {
            return;
        };
        if !title.is_empty() {
            pr.title = title;
        }
        pr.state = PrState::Creating;
        let event = WorkspaceEvent::CreatePullRequest {
            run: run.clone(),
            title: pr.title.clone(),
            body: pr.body.clone(),
            draft: pr.draft,
            keep_pushing: pr.keep_pushing,
            reviewers,
        };
        cx.emit(event);
        cx.notify();
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
                    .gap(sp(3.))
                    .pt(sp(30.))
                    .text_color(t.muted)
                    .child(ui::logo(t, 40.))
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
                    .gap(sp(3.))
                    .px(sp(if compact { 4. } else { 6. }))
                    .py(sp(if compact { 4. } else { 5. }))
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
                .px(sp(6.))
                .border_b_1()
                .border_color(t.border)
                .font_weight(weight::STRONG)
                .child("New run");
        };
        let (color, label) = ui::status_look(&run.status, t);
        let live = run.status.is_live();
        let done = self.catalog.pull_requests
            && run.status == RunStatus::Finished(StopReason::Stop);
        let id = run.id.clone();
        div()
            .h(px(48.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(sp(3.))
            .px(sp(6.))
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .typeset(Type::LEAD)
                    .font_weight(weight::STRONG)
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
                Type::CAPTION,
                t.dim,
            ))
            .child(div().flex_1())
            .child(ui::mono(
                crate::view::usd(run.usage.cost),
                Type::CAPTION,
                t.muted,
            ))
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
                            .child(ui::button(
                                "Compare forks",
                                ButtonKind::Secondary,
                                t,
                            ))
                            .on_click(cx.listener(move |ws, _, _, cx| {
                                ws.navigate(route.clone(), cx)
                            }))
                    }),
            )
            .when(done, |header| {
                header.child(
                    div()
                        .id("pull-request")
                        .child(
                            ui::button(
                                "Pull request",
                                ButtonKind::Secondary,
                                t,
                            )
                            .child(ui::icon(
                                Icon::PullRequest,
                                IconSize::COMPACT,
                                t.text_soft,
                            )),
                        )
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            ws.open_pull_request(&id, cx)
                        })),
                )
            })
            .child(
                div()
                    .id("fork")
                    .child(
                        ui::button("Fork here", ButtonKind::Secondary, t)
                            .child(ui::icon(
                                Icon::Fork,
                                IconSize::COMPACT,
                                t.text_soft,
                            )),
                    )
                    .on_click(cx.listener(|ws, _, window, cx| {
                        ws.start_fork(window, cx)
                    })),
            )
            .when(live, |header| {
                header.child(
                    div()
                        .id("cancel")
                        .child(ui::button("Cancel", ButtonKind::Danger, t))
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
            send.child(ui::round_button(
                "phone-send",
                Icon::Send,
                t.accent,
                None,
                t,
            ))
        } else {
            send.child(ui::button(
                if self.forking.is_some() {
                    "Fork"
                } else if live {
                    "Steer"
                } else {
                    "Start"
                },
                ButtonKind::Primary,
                t,
            ))
        };

        div()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .px(sp(if compact { 3. } else { 6. }))
            .pt(sp(if compact { 2.5 } else { 0. }))
            .pb(sp(if compact { 4.5 } else { 4. }))
            .when(compact, |bar| bar.border_t_1().border_color(t.border))
            .when_some(self.fork_banner(t, cx), |bar, banner| bar.child(banner))
            .when_some(queued, |bar, text| {
                bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(sp(2.))
                        .px(sp(3.))
                        .py(sp(1.75))
                        .rounded(radius::CONTROL)
                        .bg(t.blue_soft)
                        .border_1()
                        .border_color(t.blue_border)
                        .typeset(Type::CAPTION)
                        .child(ui::icon(Icon::Chevron, IconSize::COMPACT, t.blue))
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
                    .gap(sp(2.5))
                    .when(compact && live, |row| {
                        row.child(
                            ui::round_button(
                                "phone-cancel",
                                Icon::Stop,
                                t.red,
                                Some(t.red_border),
                                t,
                            )
                            .on_click(cx.listener(|ws, _, _, cx| ws.cancel(cx))),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap(sp(2.5))
                            .min_h(px(44.))
                            .px(sp(3.5))
                            .border_1()
                            .border_color(t.border_strong)
                            .rounded(if compact { radius::FULL } else { radius::LARGE })
                            .bg(t.panel)
                            .when(!compact, |field| {
                                field.child(ui::icon(Icon::Paperclip, IconSize::BASE, t.muted))
                            })
                            .child(self.composer.clone())
                            .when(!compact, |field| {
                                field.child(ui::mono(
                                    if live { "Enter steers" } else { "Enter starts" },
                                    Type::MICRO,
                                    t.dim,
                                ))
                            }),
                    )
                    .child(send),
            )
    }

    /// Above the composer while it writes a fork: the turn it forks
    /// after, with steps back and forward, and a way out.
    fn fork_banner(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Div> {
        let (run, turn) = self.forking.as_ref()?;
        let run = self.run(run)?;
        let last = Self::last_fork_turn(run).max(1);
        let step = |id: &'static str, glyph: Icon, delta: i32, on: bool| {
            div()
                .id(id)
                .size(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::TAG)
                .when(on, |button| {
                    button
                        .cursor_pointer()
                        .hover(|style| style.bg(gpui::white().opacity(0.06)))
                })
                .child(ui::icon(
                    glyph,
                    IconSize::COMPACT,
                    if on { t.blue } else { t.dim },
                ))
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.step_fork(delta, cx)),
                )
        };
        Some(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .px(sp(3.))
                .py(sp(1.25))
                .rounded(radius::CONTROL)
                .bg(t.blue_soft)
                .border_1()
                .border_color(t.blue_border)
                .typeset(Type::CAPTION)
                .child(ui::icon(Icon::Fork, IconSize::COMPACT, t.blue))
                .child(div().text_color(t.blue).child("Fork"))
                .child(
                    div()
                        .text_color(t.text_soft)
                        .child(format!("{} after turn", run.title)),
                )
                .child(step("fork-earlier", Icon::Back, -1, *turn > 1))
                .child(ui::mono(
                    format!("{turn} of {last}"),
                    Type::CAPTION,
                    t.text,
                ))
                .child(step("fork-later", Icon::Chevron, 1, *turn < last))
                .child(
                    div().flex_1().text_color(t.muted).child(
                        "The fork gets this turn's conversation and code.",
                    ),
                )
                .child(
                    div()
                        .id("fork-cancel")
                        .text_color(t.blue)
                        .cursor_pointer()
                        .hover(|style| style.underline())
                        .child("Cancel")
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.cancel_fork(cx)),
                        ),
                ),
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
                    .gap(sp(4.5))
                    .px(sp(4.))
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
                    .p(sp(4.))
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
            Route::Setup(_) | Route::PullRequest(_) => {
                self.focused(compact, t, cx)
            }
        }
    }

    fn desktop(
        &self,
        wide: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.route.is_focused() {
            return self.focused(false, t, cx);
        }
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

    /// A screen that takes the whole window.
    fn focused(
        &self,
        compact: bool,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &self.route {
            Route::Setup(step) => {
                screens::setup::render(self, *step, compact, t, cx)
            }
            Route::PullRequest(run) => {
                screens::pull_request::render(self, run, compact, t, cx)
            }
            _ => self.screen(compact, t, cx),
        }
    }

    fn phone(&self, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        if self.route.is_focused() {
            return self.focused(true, t, cx);
        }
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
                .rounded(radius::BOX)
                .typeset(Type::SMALL)
                .cursor_pointer()
                .text_color(if active { t.text } else { t.muted })
                .when(active, |segment| segment.bg(t.selected))
                .child(tab.label())
                .on_click(cx.listener(move |ws, _, _, cx| ws.set_tab(tab, cx)))
        });
        let live = run.status.is_live();
        let done = self.catalog.pull_requests
            && run.status == RunStatus::Finished(StopReason::Stop);
        let id = run.id.clone();
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
                    .gap(sp(4.))
                    .px(sp(4.))
                    .pt(sp(2.))
                    .pb(sp(6.))
                    .bg(t.panel)
                    .border_t_1()
                    .border_color(t.border_strong)
                    .rounded_t(radius::SHEET)
                    .child(
                        div().flex().justify_center().child(
                            div()
                                .w(px(40.))
                                .h(px(4.))
                                .rounded(radius::HAIRLINE)
                                .bg(t.border_strong),
                        ),
                    )
                    .child(
                        div()
                            .grid()
                            .grid_cols(Tab::ALL.len() as u16)
                            .gap(sp(1.))
                            .p(sp(1.))
                            .rounded(radius::LARGE)
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
                            .grid_cols(if live || done { 2 } else { 1 })
                            .gap(sp(2.))
                            .child(
                                div()
                                    .id("sheet-fork")
                                    .child(ui::big_button(
                                        "Fork here",
                                        None,
                                        ButtonKind::Secondary,
                                        t,
                                    ))
                                    .on_click(cx.listener(
                                        |ws, _, window, cx| {
                                            ws.start_fork(window, cx)
                                        },
                                    )),
                            )
                            .when(done, |row| {
                                row.child(
                                    div()
                                        .id("sheet-pull-request")
                                        .child(ui::big_button(
                                            "Pull request",
                                            None,
                                            ButtonKind::Secondary,
                                            t,
                                        ))
                                        .on_click(cx.listener(
                                            move |ws, _, _, cx| {
                                                ws.open_pull_request(&id, cx)
                                            },
                                        )),
                                )
                            })
                            .when(live, |row| {
                                row.child(
                                    div()
                                        .id("sheet-cancel")
                                        .child(ui::big_button(
                                            "Cancel run",
                                            None,
                                            ButtonKind::Danger,
                                            t,
                                        ))
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
        // A dialog covers the app, wherever the app is drawn.
        let body = div()
            .relative()
            .size_full()
            .child(body)
            .when_some(self.alert.clone(), |body, (title, message)| {
                body.child(ui::dialog(
                    title,
                    message,
                    div()
                        .id("alert-ok")
                        .child(ui::button("OK", ButtonKind::Primary, &t))
                        .on_click(
                            cx.listener(|ws, _, _, cx| ws.dismiss_alert(cx)),
                        ),
                    &t,
                ))
            })
            .into_any_element();
        let body = if self.phone_preview {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.backdrop)
                .child(
                    div()
                        .w(px(390.))
                        .h(px(844.))
                        .max_h_full()
                        .flex_shrink_0()
                        .overflow_hidden()
                        .rounded(radius::DEVICE)
                        .border_1()
                        .border_color(t.border_strong)
                        .bg(t.bg)
                        .child(body),
                )
                .into_any_element()
        } else if let Some((width, height)) = self.frame {
            div()
                .size_full()
                .bg(t.backdrop)
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
            .on_action(cx.listener(|ws, _: &GoBack, _, cx| ws.escape(cx)))
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
            .typeset(if phone { Type::PHONE } else { Type::SMALL })
            .child(body)
    }
}
