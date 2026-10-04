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
    ListAlignment,
    ListOffset,
    ListState,
    Subscription,
    Task,
    Window,
    actions,
    div,
    prelude::*,
    px,
    rems,
};
use tau_agent::{
    event::{RunEvent, StopReason},
    tool::RunId,
};
use tau_vcs::Landing;

use crate::{
    assets::Icon,
    catalog::{Catalog, PluginInfo},
    input::{InputEvent, TextInput},
    models::{ModelChoice, USAGE_SETTINGS_URL},
    pairing::{PairRequest, PairStep, Pairing, PairingUpdate, Progress},
    phones::{Phones, PhonesRequest},
    plan_usage::{PlanAction, PlanAlert},
    pull_request::{PrState, PullRequest},
    push::PushState,
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
        control,
        radius,
        sp,
        theme,
        weight,
    },
    ui::{
        self,
        Material as _,
        chrome,
        components::ButtonKind,
        inspector,
        screens,
        transcript,
    },
    update::HostUpdate,
    view::{
        ChildKind,
        ChildRun,
        CodeState,
        Ending,
        Item,
        LandedCard,
        LandingRecord,
        Origin,
        RunStatus,
        RunUpdate,
        RunView,
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
        ShowPlugins,
        Search,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        SlashUp,
        SlashDown,
        SlashComplete
    ]
);

const CONTEXT: &str = "Workspace";

mod composer;
mod landing;
mod layout;
mod onboarding;
mod pairing;
mod pull_request;
mod push;
mod synced;

pub use synced::Synced;

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
        KeyBinding::new("ctrl-k", Search, Some(CONTEXT)),
        // The interface's size, as a browser zooms a page.
        KeyBinding::new("ctrl-=", ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl-+", ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl-shift-=", ZoomIn, Some(CONTEXT)),
        KeyBinding::new("ctrl--", ZoomOut, Some(CONTEXT)),
        KeyBinding::new("ctrl-0", ZoomReset, Some(CONTEXT)),
        // The composer's popover; elsewhere the keys go on.
        KeyBinding::new("up", SlashUp, Some(CONTEXT)),
        KeyBinding::new("down", SlashDown, Some(CONTEXT)),
        KeyBinding::new("tab", SlashComplete, Some(CONTEXT)),
    ]);
}

/// What the user asked for. The host subscribes and acts on these.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WorkspaceEvent {
    /// Start a new run with this prompt, on this model, in this
    /// repository.
    NewRun {
        prompt: String,
        model: ModelChoice,
        repo: String,
    },
    /// The repositories the sidebar shows open, to open them the same
    /// way next time.
    OpenRepos(Vec<String>),
    /// Stop listing the repository. Its runs and project stay.
    HideRepo {
        repo: String,
    },
    /// Bring new commits into the repository from its source.
    UpdateRepo {
        repo: String,
    },
    /// Save the TypeSafe key tau-constitution checks with, or forget it.
    JevKey {
        key: Option<String>,
    },
    /// A plugin's UI asks its host half to carry out `action`.
    PluginAct {
        plugin: String,
        action: serde_json::Value,
    },
    /// Save a plugin's settings.
    PluginSettings {
        plugin: String,
        settings: serde_json::Value,
    },
    /// Store `body` as `plugin`'s record with `run`; every interface
    /// folds it once stored.
    PluginRecord {
        run: RunId,
        plugin: String,
        body: serde_json::Value,
    },
    /// Close the conversation: it leaves the sidebar (History keeps it),
    /// and stops if it is going. A message to it opens it again.
    CloseRun {
        run: RunId,
    },
    /// The person's message to a run. The host decides what it does,
    /// as the run is when it arrives: a run that is going reads it
    /// before its next turn (`Run::steer`); a finished one goes on with
    /// it, on `model`.
    Say {
        run: RunId,
        text: String,
        model: ModelChoice,
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
        model: ModelChoice,
    },
    /// Make `choice` `agent`'s model when a run does not pick one.
    SetDefaultModel {
        agent: String,
        choice: ModelChoice,
    },
    /// Leave the model out of the picker, or show it again.
    HideModel {
        id: String,
        hidden: bool,
    },
    /// Keep this branch of a fork and drop the others.
    KeepBranch {
        run: RunId,
    },
    /// Say what landing this child run on its parent would do.
    PreviewLanding {
        run: RunId,
    },
    /// Land this child run on its parent, then close it (ADR 0009).
    Land {
        run: RunId,
    },
    /// Drop this child run: abandon its own changes and close it.
    DropChild {
        run: RunId,
    },
    /// Take this chat out of its main chat's landing queue.
    Unqueue {
        run: RunId,
    },
    /// Start tau's turn on this main chat again, to resolve what its
    /// last turn left in conflict.
    ResolveAgain {
        main: RunId,
    },
    /// The person will resolve this main chat's conflicts themselves:
    /// the card goes, and the mark stays until main is clean.
    DismissConflicts {
        main: RunId,
    },
    /// Run a query against the store.
    Query {
        sql: String,
    },
    /// Pair this phone with the tau on a computer, or reach it again;
    /// answer with [`Workspace::update_pairing`].
    /// Handled on the phone, never sent up.
    #[serde(skip)]
    Pair(PairRequest),
    /// Allow phones, show the pairing code, revoke a phone: the
    /// computer's own, never sent up.
    #[serde(skip)]
    Phones(PhonesRequest),
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
    /// Forget the GitHub sign-in.
    GitHubSignOut,
    /// Go on with a run tau closing cut off, in its workspace, with
    /// tau's message saying so. The host answers with a
    /// [`HostUpdate::TauTurn`], or [`Workspace::resume_failed`].
    ResumeCutOff {
        run: RunId,
    },
    /// Sign in with ChatGPT in the browser: `account` again (its id), or
    /// a new account with `None`. `consent` asks again for plan usage.
    ChatGptSignIn {
        account: Option<String>,
        consent: bool,
    },
    /// The redirect URL of the ChatGPT sign-in in progress, pasted from
    /// the browser, for when the browser cannot reach tau.
    ChatGptCallback {
        url: String,
    },
    /// Give up on the ChatGPT sign-in waiting for the browser.
    ChatGptCancel,
    /// Sign in with this saved ChatGPT account (its id) from now on.
    SwitchChatGpt {
        account: String,
    },
    /// Sign out of the active ChatGPT account, revoking its session;
    /// runs use what is left, if anything.
    SignOut,
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
    /// Push the repository's trunk, the main chat's commits, to GitHub
    /// (ADR 0023); fetch first when `fetch`, which puts them on top of
    /// GitHub's new commits. Answer with [`HostUpdate::Pushed`].
    Push {
        repo: String,
        fetch: bool,
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

/// How many replies `run` has: its texts from the model.
fn replies(run: &RunView) -> usize {
    run.items
        .iter()
        .filter(|item| matches!(item, crate::view::Item::Text(_)))
        .count()
}

/// `run`'s transcript up to the end of `turn`, for a fork after it.
fn inherited_items(run: &RunView, turn: u32) -> Vec<crate::view::Item> {
    let end = run.items.iter().position(|item| {
        matches!(item, crate::view::Item::TurnEnd { turn: t } if *t == turn)
    });
    end.map_or_else(Vec::new, |end| run.items[..=end].to_vec())
}

/// What an onboarding step opened from the app is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupGoal {
    /// Sign in to ChatGPT.
    Model,
    /// Sign in to GitHub.
    GitHub,
    /// Clone repositories from GitHub.
    Repos,
}

/// A dialog over the app.
#[derive(Debug, Clone, PartialEq)]
pub struct Dialog {
    pub title: String,
    pub message: String,
}

/// Where a model picker's choice goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerTarget {
    /// The next run the composer starts.
    Next,
    /// The fork being written.
    Fork,
    /// An open conversation's next message.
    Run(RunId),
    /// An agent's default.
    Default(String),
}

pub struct Workspace {
    pub(crate) name: String,
    pub(crate) runs: Vec<RunView>,
    pub(crate) catalog: Catalog,
    pub(crate) route: Route,
    pub(crate) back_stack: Vec<Route>,
    /// The run the composer and the inspector act on.
    current: Option<RunId>,
    pub(crate) composer: Entity<TextInput>,
    pub(crate) history_filter: Entity<TextInput>,
    /// History's query box, and what the last query returned.
    pub(crate) query: Entity<TextInput>,
    pub(crate) query_result: Option<Result<tau_store::Table, String>>,
    /// Files attached to the next message.
    pub(crate) attachments: Vec<crate::attach::Attachment>,
    /// The search palette, and what is typed in it.
    pub(crate) searching: bool,
    pub(crate) search: Entity<TextInput>,
    /// Whether the side panel shows the event log open.
    events_open: bool,
    sheet_open: bool,
    /// Steering messages the host took that the run has not read yet,
    /// in order, per run.
    queued: HashMap<RunId, Vec<String>>,
    /// Finished runs asked to go on, with how they had ended, until they
    /// start again.
    resuming: HashMap<RunId, RunStatus>,
    /// How many of each run's replies the user has seen: the rest are
    /// unread.
    seen: HashMap<RunId, usize>,
    /// Conversations closed: not in the sidebar.
    pub(crate) closed: HashSet<RunId>,
    /// The run row under the pointer, which offers to close it.
    pub(crate) hovered_run: Option<RunId>,
    /// The composer is writing a fork of this run, after this turn.
    pub(crate) forking: Option<(RunId, u32)>,
    /// A dialog over the app: something failed.
    pub(crate) dialog: Option<Dialog>,
    /// The model the next run starts on, and whether the user picked it
    /// (else it follows coder's default).
    pub(crate) next_model: ModelChoice,
    pub(crate) next_model_picked: bool,
    /// The model the fork being written runs on.
    pub(crate) fork_model: ModelChoice,
    /// Where the open model picker writes its choice.
    pub(crate) picker: Option<PickerTarget>,
    pub(crate) model_search: Entity<TextInput>,
    /// Whether the title bar explains the open run's fixed model.
    /// The model each open conversation's next message goes to, when
    /// someone picked one other than its current model.
    pub(crate) run_models: HashMap<RunId, ModelChoice>,
    /// Whether the last layout showed the inspector, for placing the
    /// model picker beside it.
    inspector_shown: bool,
    /// Whether the person opened the run's details: the inspector,
    /// beside the transcript. Closed, the transcript has the width.
    pub(crate) details_open: bool,
    /// How large the interface is drawn: 1 is as designed. Every size
    /// is in rems, so this sets the rem.
    pub(crate) zoom: f32,
    /// Where the interface's settings are saved; none keeps the zoom
    /// for this window only.
    interface_settings: Option<std::path::PathBuf>,
    /// Which of a repository's runs its page lists.
    pub(crate) runs_filter: crate::ui::screens::repo::RunsFilter,
    /// The repository History shows the runs of; none shows all.
    pub(crate) history_repo: Option<String>,
    /// Plugin notes opened to show their detail, as `(run, item index)`.
    /// Notes start closed.
    pub(crate) open_notes: HashSet<(RunId, usize)>,
    /// Tool cards that fold, opened, as `(run, call id)`. They start
    /// closed.
    open_cards: HashSet<(RunId, String)>,
    pub(crate) kept_branch: Option<RunId>,
    /// Child runs on their way to landing, by run.
    landings: HashMap<RunId, LandingState>,
    /// Where each repository's push to GitHub stands, by repository.
    pushes: HashMap<String, PushState>,
    /// Runs that proposed their landing with `vcs_land` and have not
    /// stopped yet.
    proposed: HashSet<RunId>,
    pub(crate) setup: Setup,
    pub(crate) pairing: Pairing,
    /// The phones that may reach this computer, as the Phones screen
    /// shows them.
    pub(crate) phones: Phones,
    /// A computer's address and pairing code, typed.
    pub(crate) pair_address: Entity<TextInput>,
    pub(crate) pair_code: Entity<TextInput>,
    /// This phone's name, as the computer lists it.
    pub(crate) phone_name: Entity<TextInput>,
    pub(crate) pull_requests: HashMap<RunId, PullRequest>,
    /// The code of each comparison opened, by (run, fork).
    pub(crate) branch_code: HashMap<(RunId, RunId), CodeState>,
    pub(crate) github_token: Entity<TextInput>,
    /// Where the ChatGPT sign-in's redirect URL can be pasted.
    pub(crate) chatgpt_callback: Entity<TextInput>,
    /// What a run that stopped on the ChatGPT plan asks of the user.
    pub(crate) plan_alert: Option<PlanAlert>,
    pub(crate) repo_filter: Entity<TextInput>,
    pub(crate) first_task: Entity<TextInput>,
    pub(crate) pr_title: Entity<TextInput>,
    pub(crate) reviewers: Entity<TextInput>,
    /// The repository new runs start in: the one the sidebar last
    /// selected.
    pub(crate) repo: Option<String>,
    /// The repositories the sidebar shows open.
    pub(crate) open_repos: HashSet<String>,
    /// Repositories that list all their runs, not only the newest.
    pub(crate) all_runs: HashSet<String>,
    /// The repository row under the pointer, which shows its actions.
    pub(crate) hovered_repo: Option<String>,
    /// The repository whose menu is open.
    pub(crate) repo_menu: Option<String>,
    pub(crate) sidebar_filter: Entity<TextInput>,
    /// Why an onboarding step was opened from the app, if it was: once
    /// that is done, go back rather than on through onboarding.
    pub(crate) setup_goal: Option<SetupGoal>,
    /// What onboarding showed last frame, for its transitions.
    pub(crate) setup_motion: crate::motion::SetupMotion,
    /// The user asked for less motion: no loops, and plain fades.
    reduce_motion: bool,
    /// The dialog that asks for the TypeSafe key, with its field.
    pub(crate) adding_jev_key: bool,
    pub(crate) jev_key: Entity<TextInput>,
    /// The composer popover's selected row, the text it was dismissed
    /// for (Esc), and the text it last saw.
    pub(crate) slash_selected: usize,
    pub(crate) slash_dismissed: Option<String>,
    pub(crate) slash_seen: String,
    /// The open run's transcript: only the items in view are laid out.
    /// While `follow` holds, it is kept scrolled to the newest item.
    transcript: ListState,
    /// The run and item count `transcript` was last told about.
    listed: Option<(RunId, usize)>,
    /// The transcript's width at its last layout, read before each frame:
    /// the list cannot be asked while it lays items out.
    transcript_width: Option<gpui::Pixels>,
    /// Keep the transcript at its bottom as the run grows. Scrolling up
    /// turns it off; scrolling back down turns it on.
    follow: bool,
    focus: FocusHandle,
    /// Scripted runs playing, for demos. Several can play at once.
    replays: Vec<Task<()>>,
    /// Draw the phone layout in a phone-sized frame, whatever the width.
    phone_preview: bool,
    /// Echo what the host applies, for phones.
    mirrored: bool,
    /// Lay out at exactly this size, pinned to the top left: for
    /// comparing screens against their designs.
    frame: Option<(f32, f32)>,
    /// The window's width at the last frame, for screens that size
    /// their columns to it.
    width: gpui::Pixels,
    _subscriptions: Vec<Subscription>,
    /// The workspace itself, for plugins' handles.
    pub(crate) weak: gpui::WeakEntity<Workspace>,
    /// Each plugin's window state, by plugin (ADR 0017).
    pub(crate) plugin_ui: HashMap<String, gpui::AnyEntity>,
    /// What plugins' handles asked, to carry out after the event.
    pub(crate) plugin_requests: crate::plugins::Requests,
    /// What a plugin asked to give the keys to, as the window draws.
    pub(crate) plugin_focus: Option<gpui::FocusHandle>,
    /// Whether a plugin drew in the composer's place last frame, and
    /// whether the composer takes the keys back now that none does.
    composer_replaced: std::cell::Cell<bool>,
    composer_back: std::cell::Cell<bool>,
}

impl EventEmitter<WorkspaceEvent> for Workspace {}

/// What the host applied, echoed while [`Workspace::set_mirrored`] is
/// on: the desktop passes it on to phones.
impl EventEmitter<HostUpdate> for Workspace {}

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
        let plugin_requests = crate::plugins::Requests::default();
        let composer =
            cx.new(|cx| TextInput::new("Start a new run", cx).multiline());
        let history_filter = cx.new(|cx| {
            TextInput::new("Filter by run, repository, model or stop", cx)
        });
        let search = cx.new(|cx| {
            TextInput::new("Search runs, repositories, actions", cx)
                .keep_on_submit()
        });
        let query = cx.new(|cx| {
            let mut input =
                TextInput::new("select … from runs", cx).keep_on_submit();
            input.set_text(catalog.store.sample_query.clone(), cx);
            input
        });
        let model_search = cx.new(|cx| TextInput::new("Search models", cx));
        // Fields whose Enter submits through `take_field`, which reads
        // and clears them: they keep their text through Enter.
        let github_token = cx.new(|cx| {
            TextInput::new("github_pat_…", cx).masked().keep_on_submit()
        });
        let chatgpt_callback = cx.new(|cx| {
            TextInput::new("http://127.0.0.1:1455/auth/callback?code=…", cx)
                .masked()
                .keep_on_submit()
        });
        let repo_filter =
            cx.new(|cx| TextInput::new("Filter your repositories", cx));
        let pair_address = cx
            .new(|cx| TextInput::new("100.84.12.7:7443", cx).keep_on_submit());
        let pair_code =
            cx.new(|cx| TextInput::new("K7QM-2XPA", cx).keep_on_submit());
        let phone_name =
            cx.new(|cx| TextInput::new("Phone", cx).keep_on_submit());
        let first_task = cx.new(|cx| {
            TextInput::new(
                "Describe the task, for example: the retry loop ignores \
                 retry-after on 429s. Honor it and add tests.",
                cx,
            )
            .keep_on_submit()
        });
        let pr_title =
            cx.new(|cx| TextInput::new("Title", cx).keep_on_submit());
        let reviewers =
            cx.new(|cx| TextInput::new("@reviewer", cx).keep_on_submit());
        let sidebar_filter =
            cx.new(|cx| TextInput::new("Filter repositories and runs", cx));
        let jev_key =
            cx.new(|cx| TextInput::new("ts-…", cx).masked().keep_on_submit());
        let subscriptions = vec![
            cx.subscribe_in(
                &composer,
                window,
                |ws, _, event: &InputEvent, window, cx| match event {
                    InputEvent::Submit(text) => {
                        ws.submit_in(text.clone(), Some(window), cx)
                    }
                },
            ),
            // Its popover follows what is typed.
            cx.observe(&composer, |ws, _, cx| ws.composer_changed(cx)),
            // Filters apply as you type.
            cx.observe(&history_filter, |_, _, cx| cx.notify()),
            cx.observe(&model_search, |_, _, cx| cx.notify()),
            cx.observe(&repo_filter, |_, _, cx| cx.notify()),
            // Onboarding's loops pause while the window is in the
            // background.
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
            cx.observe(&sidebar_filter, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &search,
                window,
                |ws, _, event: &InputEvent, window, cx| {
                    let InputEvent::Submit(_) = event;
                    ws.pick_first(window, cx);
                },
            ),
            cx.observe(&search, |_, _, cx| cx.notify()),
            cx.subscribe(&query, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(_) = event;
                ws.run_query(cx);
            }),
            cx.subscribe(&jev_key, |ws, _, _: &InputEvent, cx| {
                ws.submit_jev_key(cx)
            }),
            cx.subscribe(&github_token, |ws, _, _: &InputEvent, cx| {
                ws.submit_token(cx)
            }),
            cx.subscribe(&chatgpt_callback, |ws, _, _: &InputEvent, cx| {
                ws.submit_chatgpt_callback(cx)
            }),
            // Enter in either pairing field connects.
            cx.subscribe(&pair_address, |ws, _, _: &InputEvent, cx| {
                ws.connect_typed(cx)
            }),
            cx.subscribe(&pair_code, |ws, _, _: &InputEvent, cx| {
                ws.connect_typed(cx)
            }),
            cx.subscribe(&phone_name, |ws, _, _: &InputEvent, cx| {
                ws.open_tau(cx)
            }),
            cx.subscribe(&first_task, |ws, _, _: &InputEvent, cx| {
                ws.start_first_run(cx)
            }),
        ];
        composer.read(cx).focus_handle(cx).focus(window, cx);
        let current = runs.first().map(|run| run.id.clone());
        let next_model = catalog.models.settings.default_for("coder");
        let mut workspace = Self {
            name: name.into(),
            runs,
            catalog,
            route: Route::Home,
            back_stack: Vec::new(),
            current,
            composer,
            history_filter,
            query,
            query_result: None,
            searching: false,
            search,
            attachments: Vec::new(),
            events_open: false,
            sheet_open: false,
            queued: HashMap::new(),
            resuming: HashMap::new(),
            seen: HashMap::new(),
            closed: HashSet::new(),
            hovered_run: None,
            forking: None,
            dialog: None,
            next_model,
            next_model_picked: false,
            fork_model: ModelChoice::default(),
            picker: None,
            model_search,
            run_models: HashMap::new(),
            inspector_shown: false,
            details_open: false,
            zoom: 1.,
            interface_settings: None,
            runs_filter: Default::default(),
            history_repo: None,
            open_notes: HashSet::new(),
            open_cards: HashSet::new(),
            kept_branch: None,
            landings: HashMap::new(),
            pushes: HashMap::new(),
            proposed: HashSet::new(),
            setup: Setup::default(),
            pairing: Pairing::default(),
            phones: Phones::default(),
            pair_address,
            pair_code,
            phone_name,
            pull_requests: HashMap::new(),
            branch_code: HashMap::new(),
            github_token,
            chatgpt_callback,
            plan_alert: None,
            repo_filter,
            first_task,
            pr_title,
            reviewers,
            repo: None,
            open_repos: HashSet::new(),
            all_runs: HashSet::new(),
            hovered_repo: None,
            repo_menu: None,
            sidebar_filter,
            setup_goal: None,
            setup_motion: Default::default(),
            reduce_motion: false,
            weak: cx.weak_entity(),
            plugin_ui: {
                let weak = cx.weak_entity();
                crate::plugins::registry()
                    .plugins()
                    .map(|plugin| {
                        let handle = crate::plugins::handle_for(
                            plugin.name(),
                            plugin_requests.clone(),
                            weak.clone(),
                        );
                        (plugin.name().to_owned(), plugin.new_ui(handle, cx))
                    })
                    .collect()
            },
            plugin_requests,
            plugin_focus: None,
            composer_replaced: std::cell::Cell::new(false),
            composer_back: std::cell::Cell::new(false),
            adding_jev_key: false,
            jev_key,
            slash_selected: 0,
            slash_dismissed: None,
            slash_seen: String::new(),
            transcript: ListState::new(0, ListAlignment::Top, px(600.)),
            listed: None,
            transcript_width: None,
            follow: true,
            focus: cx.focus_handle(),
            replays: Vec::new(),
            phone_preview: false,
            mirrored: false,
            frame: None,
            width: NARROW_MAX,
            _subscriptions: subscriptions,
        };
        // Scrolling up stops following new output; back at the bottom,
        // it follows again. The list is busy while it calls this, so the
        // offset is read after.
        let this = cx.entity().downgrade();
        let list = workspace.transcript.clone();
        workspace.transcript.set_scroll_handler(move |_, _, cx| {
            let (this, list) = (this.clone(), list.clone());
            cx.defer(move |cx| {
                let offset = -list.scroll_px_offset_for_scrollbar().y;
                let bottom = list.max_offset_for_scrollbar().y;
                let _ = this.update(cx, |ws, _| {
                    ws.follow = offset >= bottom - px(24.);
                });
            });
        });
        workspace.restore_repos();
        workspace.mark_all_seen();
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

    /// `plugin`'s state in this window, as its own type.
    pub fn plugin_ui<T: 'static>(&self, plugin: &str) -> Option<Entity<T>> {
        self.plugin_ui.get(plugin)?.clone().downcast().ok()
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn set_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        // The query box starts from the store's sample, until typed in.
        let typed = self.query.read(cx).text().to_owned();
        if typed.trim().is_empty() || typed == self.catalog.store.sample_query {
            let sample = catalog.store.sample_query.clone();
            self.query
                .update(cx, |input, cx| input.set_text(sample, cx));
        }
        self.closed.extend(catalog.closed_runs.iter().cloned());
        self.catalog = catalog;
        self.restore_repos();
        if !self.next_model_picked {
            self.next_model = self.catalog.models.settings.default_for("coder");
        }
        cx.notify();
    }

    /// Adds a run at the top of the list and opens it. A fork is listed
    /// under the run it came from, too.
    pub fn push_run(&mut self, mut run: RunView, cx: &mut Context<Self>) {
        let id = run.id.clone();
        // A fork is the conversation up to its turn, then its own.
        if let Origin::Fork { from, turn } = &run.origin
            && let Some(parent) = self.run(from)
        {
            let inherited = inherited_items(parent, *turn);
            run.items.splice(0..0, inherited);
            run.turn = run.turn.max(*turn);
        }
        if let Origin::Fork { from, .. } = &run.origin
            && let Some(parent) =
                self.runs.iter_mut().find(|view| &view.id == from)
        {
            parent
                .children
                .push(ChildRun::of(&run, ChildKind::Fork, None));
        }
        self.runs.insert(0, run);
        self.navigate(Route::Run(id), cx);
    }

    /// Applies what the host says, and echoes it while mirrored.
    pub fn apply(&mut self, update: HostUpdate, cx: &mut Context<Self>) {
        if self.mirrored {
            cx.emit(update.clone());
        }
        match update {
            HostUpdate::Event(event) => self.apply_event(&event, cx),
            HostUpdate::History(runs) => self.add_history(runs, cx),
            HostUpdate::Run(run) => self.push_run(*run, cx),
            HostUpdate::Catalog(catalog) => self.set_catalog(*catalog, cx),
            HostUpdate::Alert { title, message } => {
                self.show_alert(title, message, cx)
            }
            HostUpdate::PlanRefusal(refusal) => {
                self.show_plan_refusal(&refusal, cx)
            }
            HostUpdate::QueryResult(result) => {
                self.set_query_result(result, cx)
            }
            HostUpdate::PullRequest { run, pr } => {
                self.set_pull_request(&run, *pr, cx)
            }
            HostUpdate::PullRequestState { run, state } => {
                self.set_pull_request_state(&run, state, cx)
            }
            HostUpdate::Pushed { repo, result } => {
                self.pushed(&repo, result, cx)
            }
            HostUpdate::LandingPreview { run, preview } => {
                self.set_landing_preview(&run, preview, cx)
            }
            HostUpdate::Forecast { run, forecast } => {
                if let Some(view) =
                    self.runs.iter_mut().find(|view| view.id == run)
                {
                    view.forecast = forecast;
                }
                cx.notify();
            }
            HostUpdate::Landed { run, landing } => {
                self.landed(&run, landing, cx)
            }
            HostUpdate::Dropped { run, result } => {
                self.dropped(&run, result, cx)
            }
            HostUpdate::LandingQueue {
                main,
                queue,
                conflicts,
            } => self.set_landing_queue(&main, queue, conflicts, cx),
            HostUpdate::LandingFinished(record) => {
                self.landing_finished(record, cx)
            }
            HostUpdate::TauTurn { run, prompt } => {
                self.tau_turn(&run, prompt, cx)
            }
            HostUpdate::BranchCode { main, fork, code } => {
                self.set_branch_code(&main, &fork, code, cx)
            }
            HostUpdate::Steered { run, text } => {
                self.queued.entry(run).or_default().push(text);
                cx.notify();
            }
            HostUpdate::Resumed { run, prompt, model } => {
                self.resumed(&run, prompt, &model, cx)
            }
            HostUpdate::ResumeFailed(run) => self.resume_failed(&run, cx),
            HostUpdate::PluginRecord { run, plugin, body }
            | HostUpdate::PluginFold { run, plugin, body } => {
                if let Some(view) =
                    self.runs.iter_mut().find(|view| view.id == run)
                {
                    view.fold(&plugin, &body);
                }
                cx.notify();
            }
            HostUpdate::PluginRestate {
                run,
                plugin,
                records,
            } => {
                if let Some(view) =
                    self.runs.iter_mut().find(|view| view.id == run)
                {
                    view.restate(&plugin, &records);
                }
                cx.notify();
            }
            HostUpdate::PluginReply { plugin, reply } => {
                self.plugin_reply(&plugin, reply, cx);
            }
            HostUpdate::Titled { run, title } => self.retitle(&run, title, cx),
            HostUpdate::Repo { repo, main } => {
                if let Some(main) = main {
                    self.add_history(vec![*main], cx);
                }
                self.add_repo(repo, cx)
            }
            HostUpdate::Setup(update) => self.update_setup(update, cx),
            HostUpdate::Snapshot(synced) => self.restore(*synced, cx),
            HostUpdate::BranchKept(run) => {
                self.kept_branch = Some(run);
                cx.notify();
            }
            HostUpdate::Closed(run) => {
                self.closed.insert(run);
                cx.notify();
            }
        }
    }

    /// Echo what the host applies as [`HostUpdate`] events, or stop.
    pub fn set_mirrored(&mut self, mirrored: bool) {
        self.mirrored = mirrored;
    }

    /// What this workspace shows, for an interface that just connected.
    pub fn snapshot(&self) -> HostUpdate {
        HostUpdate::Snapshot(Box::new(self.synced()))
    }

    /// Shows `runs` in place of the ones shown, as a host that
    /// reconnected has them; the screen stays if its run is still there.
    fn replace_runs(&mut self, runs: Vec<RunView>, cx: &mut Context<Self>) {
        self.runs = runs;
        if self
            .current
            .as_ref()
            .is_none_or(|id| self.run(id).is_none())
        {
            self.current = self.runs.first().map(|run| run.id.clone());
        }
        if self.route.run().is_some_and(|id| self.run(id).is_none()) {
            self.back_stack.clear();
            self.route = Route::Home;
            self.entered(cx);
        }
        self.listed = None;
        cx.notify();
    }

    /// Adds runs from earlier sessions below the ones already shown.
    pub fn add_history(&mut self, runs: Vec<RunView>, cx: &mut Context<Self>) {
        for run in runs {
            if self.run(&run.id).is_none() {
                // What happened while tau was closed is not news.
                self.seen.insert(run.id.clone(), replies(&run));
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
        // A sub-agent is a chat of its own (ADR 0009), started on the
        // task its parent's call handed it.
        if let RunEvent::RunStart {
            run,
            parent: Some(parent),
            agent,
            call,
        } = event
            && self.run(run).is_none()
            && let Some(view) = self.run(parent)
        {
            // The call may be the model's or one a tool made, such as a
            // codemode script's.
            let task = call
                .as_deref()
                .and_then(|call| view.call(call))
                .filter(|(tool, _)| *tool == tau_vcs::details::DELEGATE)
                .and_then(|(_, args)| args.get("task")?.as_str())
                .map(str::to_owned)
                .unwrap_or_default();
            let mut child = RunView::new(
                run.clone(),
                crate::titles::placeholder(&task),
                agent.to_string(),
                view.model.clone(),
            )
            .in_repo(view.repo.clone())
            .with_origin(Origin::SubAgent {
                parent: parent.clone(),
            });
            child.update(RunUpdate::User(task));
            self.runs.push(child);
        }
        for run in &mut self.runs {
            run.apply(event);
        }
        // A main chat's new run puts away the card of its last push.
        if let RunEvent::RunStart { run, .. } = event
            && let Some(repo) = self.main_repo(run).map(str::to_owned)
        {
            self.pushes.retain(|pushing, state| {
                *pushing != repo || matches!(state, PushState::Pushing { .. })
            });
        }
        // A fork that finishes waits in its parent's chat, to land or
        // be dropped.
        if let RunEvent::RunEnd { run, .. } = event
            && let Some(Origin::Fork { from, .. }) =
                self.run(run).map(|view| view.origin.clone())
            && let Some(parent) =
                self.runs.iter_mut().find(|view| view.id == from)
        {
            parent.fork_finished(run);
        }
        // A sub-agent closes after landing or an intentional drop, not
        // when a failed finalization retained its workspace for recovery.
        // Its siblings wait for their own calls, including nested calls.
        if let RunEvent::ToolEnd {
            run,
            call_id,
            output,
            ..
        } = event
            && !output
                .details
                .as_ref()
                .is_some_and(|details| details["workspace_retained"] == true)
            && let Some(view) = self.run(run)
            && view
                .call(call_id)
                .is_some_and(|(tool, _)| tool == tau_vcs::details::DELEGATE)
        {
            let done: Vec<RunId> = view
                .children
                .iter()
                .filter(|child| {
                    child.kind == ChildKind::SubAgent
                        && !child.status.is_live()
                        && child.call.as_deref() == Some(call_id.as_str())
                })
                .map(|child| child.id.clone())
                .filter(|child| !self.closed.contains(child))
                .collect();
            let parent = run.clone();
            for child in done {
                self.closed.insert(child.clone());
                cx.emit(WorkspaceEvent::CloseRun { run: child.clone() });
                if self.route.run() == Some(&child) {
                    self.navigate(Route::Run(parent.clone()), cx);
                }
            }
        }
        // What it did not read, the host sends again if the run stopped
        // on its own, as a message that resumes it.
        if let RunEvent::RunEnd { run, .. } = event {
            self.queued.remove(run);
        }
        match event {
            // The run read one message it was steered with: it is in the
            // transcript now.
            RunEvent::Steered { run, text } => {
                if let Some(queued) = self.queued.get_mut(run)
                    && let Some(at) = queued.iter().position(|q| q == text)
                {
                    queued.remove(at);
                    if queued.is_empty() {
                        self.queued.remove(run);
                    }
                }
            }
            RunEvent::RunStart { run, .. } => {
                self.resuming.remove(run);
            }
            // Top-level or nested, at any depth: a script's `vcs_land`
            // proposes as the model's does. A set, so a nested call and
            // the call that made it propose once.
            RunEvent::ToolEnd { run, call_id, .. }
                if self
                    .run(run)
                    .is_some_and(|view| view.proposes_landing(call_id)) =>
            {
                self.proposed.insert(run.clone());
            }
            // A run that proposed its landing stopped: its landing card
            // opens for the person to confirm (ADR 0014).
            RunEvent::RunEnd {
                run,
                stop: StopReason::Stop,
                ..
            } if self.proposed.remove(run) => {
                self.preview_landing(run, cx);
            }
            _ => {}
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
        // What a run streamed is the host's, for every interface.
        if let RunUpdate::Event(event) = update {
            self.apply(HostUpdate::Event(event), cx);
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

    /// The current screen's name, for the title bar and the phone's
    /// header: a plugin's page names itself.
    pub(crate) fn route_title(&self, cx: &mut App) -> String {
        match &self.route {
            Route::Plugin {
                plugin,
                page,
                params,
            } => self
                .plugin_page_title(plugin, page, params, cx)
                .unwrap_or_else(|| page.clone()),
            route => route.title().to_owned(),
        }
    }

    /// Whether the window has the phone's layout.
    pub(crate) fn compact(&self) -> bool {
        self.phone_preview || self.width < PHONE_MAX
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
        // The screen's repository becomes the selected one, open in the
        // sidebar so the selection shows.
        let repo = match (&self.route, self.route.run()) {
            (route, _) if route.repo().is_some() => {
                route.repo().map(str::to_owned)
            }
            (_, Some(run)) => {
                self.run(run).map(|run| self.repo_of(run).to_owned())
            }
            _ => None,
        };
        if let Some(repo) =
            repo.filter(|repo| self.catalog.repo(repo).is_some())
        {
            if self.open_repos.insert(repo.clone()) {
                self.emit_open_repos(cx);
            }
            self.repo = Some(repo);
        }
        if !matches!(self.route, Route::Setup(_)) {
            self.setup_goal = None;
        }
        // Leaving a pairing half done stops it.
        let pairing = matches!(
            self.route,
            Route::Pair(PairStep::Scan | PairStep::Address | PairStep::Paired)
        );
        let waiting = self.pairing.busy()
            || matches!(self.pairing.progress, Progress::Compare { .. });
        if !pairing && waiting {
            self.pairing.progress = Progress::Idle;
            cx.emit(WorkspaceEvent::Pair(PairRequest::Cancel));
        }
        self.mark_open_seen();
        self.repo_menu = None;
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
        let link = plugin.page.clone()?.from(&plugin.name);
        self.link_route(&link, None)
    }

    /// The route a plugin's link opens, its empty parameters filled from
    /// what is at hand: `run`, else the current run, and the selected
    /// repository.
    pub fn link_route(
        &self,
        link: &tau_ui_plugin::Link,
        run: Option<&RunId>,
    ) -> Option<Route> {
        let mut params = link.params.clone();
        for (name, value) in &mut params {
            if !value.is_empty() {
                continue;
            }
            *value = match name.as_str() {
                "run" => run.or(self.current.as_ref())?.0.to_string(),
                "repo" => self.selected_repo()?.to_owned(),
                _ => continue,
            };
        }
        Some(Route::Plugin {
            plugin: link.plugin.clone()?,
            page: link.page.clone(),
            params,
        })
    }

    /// The screen for a plugin by name, about one run.
    pub fn plugin_route_named(&self, name: &str, run: &RunId) -> Option<Route> {
        let plugin = self.catalog.plugins.iter().find(|p| p.name == name)?;
        let link = plugin.page.clone()?.from(&plugin.name);
        self.link_route(&link, Some(run))
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
        self.composer.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Opens or closes the detail of the note at `index` of `run`.
    pub fn toggle_note(
        &mut self,
        run: &RunId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), index);
        if !self.open_notes.remove(&key) {
            self.open_notes.insert(key);
        }
        cx.notify();
    }

    pub fn note_open(&self, run: &RunId, index: usize) -> bool {
        self.open_notes.contains(&(run.clone(), index))
    }

    /// Opens or closes the folding card (a log, diff or show) of
    /// `call_id` in `run`.
    pub fn toggle_card(
        &mut self,
        run: &RunId,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), call_id.to_owned());
        if !self.open_cards.remove(&key) {
            self.open_cards.insert(key);
        }
        cx.notify();
    }

    pub fn card_open(&self, run: &RunId, call_id: &str) -> bool {
        self.open_cards.contains(&(run.clone(), call_id.to_owned()))
    }

    /// Runs what the query box holds against the store.
    pub fn run_query(&mut self, cx: &mut Context<Self>) {
        let sql = self.query.read(cx).text().trim().to_owned();
        if sql.is_empty() {
            return;
        }
        cx.emit(WorkspaceEvent::Query { sql });
    }

    /// What the store returned for the query, or why it did not.
    pub fn set_query_result(
        &mut self,
        result: Result<tau_store::Table, String>,
        cx: &mut Context<Self>,
    ) {
        self.query_result = Some(result);
        cx.notify();
    }

    pub fn query_result(&self) -> Option<&Result<tau_store::Table, String>> {
        self.query_result.as_ref()
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

    /// The last turn of `run` a new chat can start from: its last
    /// finished turn.
    pub fn last_fork_turn_of(run: &RunView) -> u32 {
        Self::last_fork_turn(run)
    }

    pub(crate) fn shows_inspector(&self) -> bool {
        self.inspector_shown
    }

    /// The smallest and largest zoom, and a step of it.
    pub const ZOOM_RANGE: (f32, f32) = (0.5, 2.);
    pub const ZOOM_STEP: f32 = 0.1;

    /// Keeps the zoom in the interface settings at `path`, starting from
    /// the one saved there.
    pub fn use_interface_settings(
        &mut self,
        path: std::path::PathBuf,
        cx: &mut Context<Self>,
    ) {
        if let Some(zoom) = crate::motion::saved_zoom(&path) {
            self.zoom = zoom.clamp(Self::ZOOM_RANGE.0, Self::ZOOM_RANGE.1);
            cx.notify();
        }
        self.interface_settings = Some(path);
    }

    /// Zooms by `steps` steps, or back to as designed for none, and
    /// saves it.
    pub fn zoom_by(&mut self, steps: i32, cx: &mut Context<Self>) {
        let zoom = if steps == 0 {
            1.
        } else {
            self.zoom + Self::ZOOM_STEP * steps as f32
        };
        // In tenths, so steps land on round sizes.
        let zoom = ((zoom * 10.).round() / 10.)
            .clamp(Self::ZOOM_RANGE.0, Self::ZOOM_RANGE.1);
        if zoom == self.zoom {
            return;
        }
        self.zoom = zoom;
        if let Some(path) = &self.interface_settings
            && let Err(error) = crate::motion::save_zoom(path, zoom)
        {
            eprintln!(
                "tau-ui: cannot save the zoom to {}: {error}",
                path.display()
            );
        }
        cx.notify();
    }

    /// Opens or closes the run's details beside the transcript.
    pub(crate) fn toggle_details(&mut self, cx: &mut Context<Self>) {
        self.details_open = !self.details_open;
        cx.notify();
    }

    /// Whether `run` can be forked at all. Runs nest one level (ADR
    /// 0016): a repository's main chat has chats under it, and those,
    /// like sub-agents, have nothing under them.
    pub fn can_fork(&self, run: &RunView) -> bool {
        self.is_main(&run.id)
    }

    /// Whether a fork can start after `turn` of `run`: the run can be
    /// forked, and the turn has ended.
    pub fn can_fork_at(&self, run: &RunView, turn: u32) -> bool {
        self.can_fork(run) && turn >= 1 && turn <= Self::last_fork_turn(run)
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
        if self.start_fork_at(run, turn, cx) {
            self.composer.read(cx).focus_handle(cx).focus(window, cx);
        }
    }

    /// Fork mode for `run` after `turn`, on the run's model, without
    /// moving the keyboard focus. Returns whether it started.
    pub fn start_fork_at(
        &mut self,
        run: &RunId,
        turn: u32,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(run) = self.run(run).filter(|run| self.can_fork(run)) else {
            return false;
        };
        let (id, turn) = (
            run.id.clone(),
            turn.clamp(1, Self::last_fork_turn(run).max(1)),
        );
        let model = Self::model_of(run);
        if !matches!(self.route, Route::Run(_) | Route::Home) {
            self.navigate(Route::Run(id.clone()), cx);
        }
        self.fork_model = model;
        self.forking = Some((id, turn));
        self.sheet_open = false;
        self.composer.update(cx, |input, cx| {
            input.clear(cx);
            input.set_placeholder("What should the fork try instead?");
        });
        cx.notify();
        true
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

    pub fn sheet_is_open(&self) -> bool {
        self.sheet_open
    }

    pub fn toggle_sheet(&mut self, cx: &mut Context<Self>) {
        self.sheet_open = !self.sheet_open;
        cx.notify();
    }

    /// Opens or closes the side panel's event log.
    pub fn toggle_events(&mut self, cx: &mut Context<Self>) {
        self.events_open = !self.events_open;
        cx.notify();
    }

    pub fn events_open(&self) -> bool {
        self.events_open
    }

    /// Submits `text` as if typed into the composer: it steers the open
    /// run if one is live, or starts a new run.
    /// What the composer holds.
    /// Whether the transcript keeps to its newest output; scrolling up
    /// stops it.
    pub fn follows(&self) -> bool {
        self.follow
    }

    pub fn composer_text<'a>(&self, cx: &'a gpui::App) -> &'a str {
        self.composer.read(cx).text()
    }

    /// Puts `text` in the composer, as if typed.
    pub fn set_composer(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = text.to_owned();
        self.composer
            .update(cx, |input, cx| input.set_text(text, cx));
        self.composer_changed(cx);
    }

    pub fn submit_prompt(&mut self, text: String, cx: &mut Context<Self>) {
        self.submit(text, cx);
    }

    fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        self.submit_in(text, None, cx);
    }

    /// Sends the composer's `text`, or runs it when it is a command.
    fn submit_in(
        &mut self,
        text: String,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        if self.run_slash(&text, window, cx) {
            return;
        }
        self.send(text, cx);
    }

    pub(crate) fn send(&mut self, text: String, cx: &mut Context<Self>) {
        let text = self.with_attachments(text);
        if let Some((run, turn)) = self.forking.take() {
            cx.emit(WorkspaceEvent::Fork {
                run,
                turn: Some(turn),
                prompt: text,
                model: self.fork_model.clone(),
            });
            self.sync_placeholder(cx);
            cx.notify();
            return;
        }
        match self.current().filter(|_| self.route != Route::NewRun) {
            Some(run) => {
                let run = run.id.clone();
                self.say(&run, text, cx);
            }
            _ => cx.emit(WorkspaceEvent::NewRun {
                prompt: text,
                model: self.next_model.clone(),
                repo: self.selected_repo().unwrap_or_default().to_owned(),
            }),
        }
        cx.notify();
    }

    /// Sends `text` to `run`, with the model picked for it or the one it
    /// was on, should it go on. It shows once the host takes it
    /// ([`HostUpdate::Steered`] or [`HostUpdate::Resumed`]), on every
    /// interface alike.
    pub(crate) fn say(
        &mut self,
        run: &RunId,
        text: String,
        cx: &mut Context<Self>,
    ) {
        // A chat that landed or was dropped takes no more messages.
        let Some(on) = self
            .run(run)
            .filter(|view| view.ending.is_none())
            .map(Self::model_of)
        else {
            return;
        };
        let model = self.run_models.remove(run).unwrap_or(on);
        self.follow = true;
        cx.emit(WorkspaceEvent::Say {
            run: run.clone(),
            text,
            model,
        });
    }

    /// The host took `prompt` for `run`: it shows, the run moves to the
    /// top of the list, and it opens again if it was closed.
    fn resumed(
        &mut self,
        run: &RunId,
        prompt: String,
        model: &ModelChoice,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.runs.iter().position(|view| &view.id == run) else {
            return;
        };
        let mut view = self.runs.remove(at);
        view.switch_model(&model.model, model.effort.label());
        self.closed.remove(run);
        self.resuming.insert(run.clone(), view.status.clone());
        view.push_user(prompt);
        view.status = RunStatus::Planning;
        self.runs.insert(0, view);
        self.after_update(cx);
    }

    /// The host could not go on with the run: it ends as it had, without
    /// the message.
    /// Names `run` `title`, in the list and under its parent.
    pub fn retitle(
        &mut self,
        run: &RunId,
        title: String,
        cx: &mut Context<Self>,
    ) {
        for view in &mut self.runs {
            if &view.id == run {
                view.title.clone_from(&title);
            }
            for child in &mut view.children {
                if &child.id == run {
                    child.title.clone_from(&title);
                }
            }
        }
        cx.notify();
    }

    pub fn resume_failed(&mut self, run: &RunId, cx: &mut Context<Self>) {
        let Some(status) = self.resuming.remove(run) else {
            return;
        };
        if let Some(view) = self.runs.iter_mut().find(|view| &view.id == run) {
            view.status = status;
            if matches!(view.items.last(), Some(crate::view::Item::User(_))) {
                view.items.pop();
            }
        }
        self.after_update(cx);
    }

    /// Whether the composer's message goes on with the open, finished
    /// run.
    pub(crate) fn continues_chat(&self) -> bool {
        self.forking.is_none()
            && self.route != Route::NewRun
            && self.current().is_some_and(|run| !run.status.is_live())
    }

    pub(crate) fn submit_from_button(&mut self, cx: &mut Context<Self>) {
        let text = self.composer.read(cx).text().trim().to_owned();
        if !text.is_empty() {
            self.composer.update(cx, |input, cx| input.clear(cx));
            self.submit(text, cx);
        }
    }

    fn after_update(&mut self, cx: &mut Context<Self>) {
        self.mark_open_seen();
        self.sync_placeholder(cx);
        cx.notify();
    }

    /// The replies of `run` the user has not seen: the run's pill.
    pub fn unread(&self, run: &RunView) -> usize {
        replies(run)
            .saturating_sub(self.seen.get(&run.id).copied().unwrap_or(0))
    }

    fn mark_all_seen(&mut self) {
        for run in &self.runs {
            self.seen.insert(run.id.clone(), replies(run));
        }
    }

    /// The run on screen has nothing unread.
    fn mark_open_seen(&mut self) {
        let open = match &self.route {
            Route::Run(id) => Some(id.clone()),
            Route::Home => self.current.clone(),
            _ => None,
        };
        if let Some(run) = open.and_then(|id| self.run(&id)) {
            self.seen.insert(run.id.clone(), replies(run));
        }
    }

    /// Closes a conversation: it leaves the sidebar, and stops if it is
    /// going. History keeps it, and a message to it opens it again.
    /// Closes a conversation from its phone screen, which has no
    /// sidebar to hover: back to the list of conversations.
    pub fn close_run_to_list(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.close_run(run, cx);
        self.back_stack.clear();
        self.route = Route::Home;
        self.entered(cx);
        cx.notify();
    }

    pub fn close_run(&mut self, run: &RunId, cx: &mut Context<Self>) {
        // A repository's main chat stays open.
        if self.is_main(run) {
            return;
        }
        if self.run(run).is_some_and(|view| view.status.is_live()) {
            cx.emit(WorkspaceEvent::Cancel { run: run.clone() });
        }
        // It leaves the sidebar once the host takes it
        // (`HostUpdate::Closed`), on every interface alike.
        self.hovered_run = None;
        cx.emit(WorkspaceEvent::CloseRun { run: run.clone() });
        if self.route.run() == Some(run) || self.current.as_ref() == Some(run) {
            // Show the next open conversation, or a new one.
            let next = self
                .runs
                .iter()
                .find(|view| {
                    &view.id != run
                        && !self.closed.contains(&view.id)
                        && view.origin == Origin::Root
                })
                .map(|view| view.id.clone());
            self.current = next.clone();
            self.back_stack.clear();
            self.route = match next {
                Some(next) => Route::Run(next),
                None => Route::NewRun,
            };
            self.entered(cx);
        }
        cx.notify();
    }

    pub fn is_closed(&self, run: &RunId) -> bool {
        self.closed.contains(run)
    }

    pub fn hover_run(
        &mut self,
        run: &RunId,
        hovered: bool,
        cx: &mut Context<Self>,
    ) {
        let now = if hovered {
            Some(run.clone())
        } else if self.hovered_run.as_ref() == Some(run) {
            None
        } else {
            return;
        };
        if self.hovered_run != now {
            self.hovered_run = now;
            cx.notify();
        }
    }

    fn sync_placeholder(&mut self, cx: &mut Context<Self>) {
        let live = self.route != Route::NewRun && self.is_live();
        let forking = self.forking.is_some();
        let continues = self.continues_chat();
        self.composer.update(cx, |input, _| {
            input.set_placeholder(if forking {
                "What should the fork try instead?"
            } else if live {
                "Steer the run, or @ a file, agent or checkpoint"
            } else if continues {
                "Reply to go on with this run"
            } else {
                "Start a new run"
            })
        });
    }

    fn is_live(&self) -> bool {
        self.current().is_some_and(|run| run.status.is_live())
    }

    // Onboarding.

    // Phones, on the computer.

    // Pairing a phone.

    /// Asks for less motion: onboarding's loops, ripples and comets
    /// stop, and its transitions become plain fades.
    pub fn set_reduce_motion(&mut self, reduce: bool, cx: &mut Context<Self>) {
        self.reduce_motion = reduce;
        cx.notify();
    }

    pub fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    /// Shows a dialog: something failed that the user asked for.
    pub fn show_alert(
        &mut self,
        title: impl Into<String>,
        message: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(Dialog {
            title: title.into(),
            message: message.into(),
        });
        cx.notify();
    }

    /// The open dialog's title and message.
    pub fn alert(&self) -> Option<(&str, &str)> {
        self.dialog
            .as_ref()
            .map(|dialog| (dialog.title.as_str(), dialog.message.as_str()))
    }

    pub fn dismiss_alert(&mut self, cx: &mut Context<Self>) {
        self.dialog = None;
        cx.notify();
    }

    /// Esc: closes a dialog if one is open, else goes back.
    pub fn escape(&mut self, cx: &mut Context<Self>) {
        if self.slash_dismiss(cx) {
        } else if self.dialog.is_some() {
            self.dismiss_alert(cx);
        } else if self.plan_alert.take().is_some() {
            cx.notify();
        } else if self.searching {
            self.close_search(cx);
        } else if self.adding_jev_key {
            self.adding_jev_key = false;
            cx.notify();
        } else if self.repo_menu.is_some() {
            self.repo_menu = None;
            cx.notify();
        } else if self.picker.is_some() {
            self.close_picker(cx);
        } else {
            self.back(cx);
        }
    }

    // Pull requests.

    // Layouts.
}

/// Where landing a child run stands.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LandingState {
    /// Waiting for the host's preview.
    Previewing,
    /// What landing would do, or why it cannot.
    Preview(Result<Landing, String>),
    /// Landing now.
    Landing,
    /// Asking before dropping the child.
    ConfirmDrop,
    /// Dropping now.
    Dropping,
}
