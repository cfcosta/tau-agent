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
};
use tau_agent::{
    event::{RunEvent, StopReason},
    tool::RunId,
};
use tau_vcs::Landing;

use crate::{
    assets::Icon,
    catalog::{Catalog, PluginInfo, PluginScreen},
    input::{InputEvent, TextInput},
    models::{ModelChoice, ModelSettings, USAGE_SETTINGS_URL},
    pairing::{PairRequest, PairStep, Pairing, PairingUpdate, Progress},
    phones::{Phones, PhonesRequest},
    plan_usage::{self, PlanAction, PlanAlert},
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
        control,
        radius,
        sp,
        theme,
        weight,
    },
    ui::{
        self,
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
        Item,
        LandedCard,
        LandingRecord,
        Merge,
        MergeRecord,
        MergedCard,
        Origin,
        Proposal,
        RunStatus,
        RunUpdate,
        RunView,
        ToolBody,
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
        ShowPlugins,
        Search,
        SlashUp,
        SlashDown,
        SlashComplete
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
        KeyBinding::new("ctrl-k", Search, Some(CONTEXT)),
        // The composer's popover; elsewhere the keys go on.
        KeyBinding::new("up", SlashUp, Some(CONTEXT)),
        KeyBinding::new("down", SlashDown, Some(CONTEXT)),
        KeyBinding::new("tab", SlashComplete, Some(CONTEXT)),
    ]);
}

/// What the user asked for. The host subscribes and acts on these.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum WorkspaceEvent {
    /// Change a conversation's goal: pause, resume, extend or clear it.
    /// The host stores the record for tau-goal, which reads it at its
    /// next check.
    Goal {
        run: RunId,
        record: tau_goal::Record,
    },
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
    /// Add the checkout at `path` as a repository; answer with
    /// [`Workspace::add_repo`], or [`Workspace::show_alert`].
    AddRepo {
        path: String,
    },
    /// Stop listing the repository. Its runs and project stay.
    HideRepo {
        repo: String,
    },
    /// Bring new commits into the repository from its source.
    UpdateRepo {
        repo: String,
    },
    /// Add a rule to the repository's constitution; `on` names where it
    /// applies (`edit.newText`, `final answer`).
    AddRule {
        repo: String,
        text: String,
        on: Vec<String>,
        review: f64,
        block: f64,
    },
    /// Rewrite rule `id` in place.
    UpdateRule {
        repo: String,
        id: String,
        text: String,
        on: Vec<String>,
        review: f64,
        block: f64,
    },
    /// Ask Jev what a rule being written makes of past calls and
    /// answers; the host answers with `Workspace::set_rule_trial`.
    TryRule {
        repo: String,
        text: String,
        on: Vec<String>,
        review: f64,
        block: f64,
        calls: Vec<(String, serde_json::Value)>,
        answers: Vec<String>,
    },
    RemoveRule {
        repo: String,
        id: String,
    },
    /// Save the TypeSafe key tau-constitution checks with, or forget it.
    JevKey {
        key: Option<String>,
    },
    /// Close the conversation: it leaves the sidebar (History keeps it),
    /// and stops if it is going. A message to it opens it again.
    CloseRun {
        run: RunId,
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
        model: ModelChoice,
    },
    /// Keep the user's model choices: defaults and hidden models.
    SaveModelSettings(ModelSettings),
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
    /// Say what merging this top-level run into trunk would do.
    PreviewMerge {
        run: RunId,
    },
    /// Merge this top-level run into trunk (ADR 0014); answer with
    /// [`Workspace::merged`].
    Merge {
        run: RunId,
    },
    /// Keep a note a memory plugin suggested.
    KeepNote {
        run: RunId,
        title: String,
    },
    /// A flagged call was looked at and found fine: it leaves the review
    /// queue for good.
    Reviewed {
        run: RunId,
        call_id: String,
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
    /// Go on with a finished run, as a chat goes on: the same run gets
    /// `prompt`, on its own model. Its events carry on in its view; if it
    /// cannot, answer with [`Workspace::resume_failed`].
    Resume {
        run: RunId,
        prompt: String,
        model: ModelChoice,
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
    pub(crate) memory_search: Entity<TextInput>,
    /// Whether the side panel shows the event log open.
    events_open: bool,
    sheet_open: bool,
    /// Steering messages sent but not yet seen by the run, per run.
    queued: HashMap<RunId, String>,
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
    /// Notes kept, per run, by title.
    pub(crate) kept: HashMap<RunId, HashSet<String>>,
    /// Plugin notes opened to show their detail, as `(run, item index)`.
    /// Notes start closed.
    pub(crate) open_notes: HashSet<(RunId, usize)>,
    /// Tool cards that fold, opened, as `(run, call id)`. They start
    /// closed.
    open_cards: HashSet<(RunId, String)>,
    /// The terminals of `bash` cards, and how each is open.
    pub(crate) terms: std::cell::RefCell<crate::ui::term_card::TermCards>,
    /// Files opened to their hunks in a diff or show card, as `(run,
    /// call id, path)`.
    open_files: HashSet<(RunId, String, String)>,
    /// The change picked in each `vcs_log` card, by full change id.
    picked_changes: HashMap<(RunId, String), String>,
    /// Flagged calls someone looked at, as `(run, call id)`.
    pub(crate) dismissed: HashSet<(RunId, String)>,
    pub(crate) kept_branch: Option<RunId>,
    /// Child runs on their way to landing, by run.
    landings: HashMap<RunId, LandingState>,
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
    /// The dialog that adds a repository, with its path field.
    pub(crate) adding_repo: bool,
    /// Why an onboarding step was opened from the app, if it was: once
    /// that is done, go back rather than on through onboarding.
    pub(crate) setup_goal: Option<SetupGoal>,
    /// What onboarding showed last frame, for its transitions.
    pub(crate) setup_motion: crate::motion::SetupMotion,
    /// The user asked for less motion: no loops, and plain fades.
    reduce_motion: bool,
    pub(crate) repo_path: Entity<TextInput>,
    /// The new rule's text and where it applies, on the Constitution
    /// screen.
    pub(crate) rule_text: Entity<TextInput>,
    pub(crate) rule_on: Entity<TextInput>,
    /// The rule editor, the Constitution screen's list, and the rule
    /// whose ⋯ menu is open.
    pub(crate) rule_draft: Option<crate::rule_editor::RuleDraft>,
    pub(crate) rules_tab: crate::rule_editor::RulesTab,
    pub(crate) rule_menu: Option<String>,
    /// The dialog that asks for the TypeSafe key, with its field.
    pub(crate) adding_jev_key: bool,
    pub(crate) jev_key: Entity<TextInput>,
    /// The composer popover's selected row, the text it was dismissed
    /// for (Esc), and the text it last saw.
    pub(crate) slash_selected: usize,
    pub(crate) slash_dismissed: Option<String>,
    pub(crate) slash_seen: String,
    /// The `/goal` popover's limits.
    pub(crate) goal_continuations: Entity<TextInput>,
    pub(crate) goal_budget: Entity<TextInput>,
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
        let composer =
            cx.new(|cx| TextInput::new("Start a new run", cx).multiline());
        let history_filter = cx.new(|cx| {
            TextInput::new("Filter by run, repository, model or stop", cx)
        });
        let memory_search = cx.new(|cx| TextInput::new("Search notes", cx));
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
        let github_token =
            cx.new(|cx| TextInput::new("github_pat_…", cx).masked());
        let chatgpt_callback = cx.new(|cx| {
            TextInput::new("http://127.0.0.1:1455/auth/callback?code=…", cx)
                .masked()
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
        });
        let pr_title =
            cx.new(|cx| TextInput::new("Title", cx).keep_on_submit());
        let reviewers =
            cx.new(|cx| TextInput::new("@reviewer", cx).keep_on_submit());
        let sidebar_filter =
            cx.new(|cx| TextInput::new("Filter repositories and runs", cx));
        let repo_path = cx.new(|cx| {
            TextInput::new("~/Code/you/project", cx).keep_on_submit()
        });
        let rule_text = cx.new(|cx| {
            TextInput::new(
                "A rule in plain words: \"No unwrap or expect outside tests.\"",
                cx,
            )
            .keep_on_submit()
        });
        let rule_on = cx.new(|cx| {
            TextInput::new("tool.field, such as grep.pattern", cx)
                .keep_on_submit()
        });
        let jev_key = cx.new(|cx| TextInput::new("ts-…", cx).masked());
        let limit = |text: String, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut input = TextInput::new("", cx).keep_on_submit();
                input.set_text(text, cx);
                input
            })
        };
        let goal_continuations =
            limit(tau_goal::DEFAULT_CONTINUATIONS.to_string(), cx);
        let goal_budget = limit(format!("{:.2}", tau_goal::DEFAULT_BUDGET), cx);
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
            // The rule editor: Enter in the rule saves it, Enter in the
            // other field adds it; what is missing updates as you type.
            cx.subscribe(&rule_text, |ws, _, _: &InputEvent, cx| {
                ws.save_rule(cx)
            }),
            cx.subscribe(&rule_on, |ws, _, _: &InputEvent, cx| {
                ws.add_other_place(cx);
            }),
            cx.observe(&rule_text, |_, _, cx| cx.notify()),
            // Its popover follows what is typed.
            cx.observe(&composer, |ws, _, cx| ws.composer_changed(cx)),
            cx.subscribe(&goal_continuations, |ws, _, _: &InputEvent, cx| {
                ws.submit_from_button(cx)
            }),
            cx.subscribe(&goal_budget, |ws, _, _: &InputEvent, cx| {
                ws.submit_from_button(cx)
            }),
            // Filters apply as you type.
            cx.observe(&history_filter, |_, _, cx| cx.notify()),
            cx.observe(&memory_search, |_, _, cx| cx.notify()),
            cx.observe(&model_search, |_, _, cx| cx.notify()),
            cx.observe(&repo_filter, |_, _, cx| cx.notify()),
            // Onboarding's loops pause while the window is in the
            // background.
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
            cx.observe(&sidebar_filter, |_, _, cx| cx.notify()),
            cx.subscribe(&repo_path, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(path) = event;
                ws.submit_repo_path(path.clone(), cx);
            }),
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
            cx.subscribe(&jev_key, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(key) = event;
                ws.submit_jev_key(key.clone(), cx);
            }),
            cx.subscribe(&github_token, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(token) = event;
                ws.submit_token(token.clone(), cx);
            }),
            cx.subscribe(&chatgpt_callback, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(url) = event;
                ws.submit_chatgpt_callback(url.clone(), cx);
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
            cx.subscribe(&first_task, |ws, _, event: &InputEvent, cx| {
                let InputEvent::Submit(task) = event;
                ws.start_first_run(task.clone(), cx);
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
            memory_search,
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
            kept: HashMap::new(),
            open_notes: HashSet::new(),
            open_cards: HashSet::new(),
            terms: Default::default(),
            open_files: HashSet::new(),
            picked_changes: HashMap::new(),
            dismissed: HashSet::new(),
            kept_branch: None,
            landings: HashMap::new(),
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
            adding_repo: false,
            setup_goal: None,
            setup_motion: Default::default(),
            reduce_motion: false,
            repo_path,
            rule_text,
            rule_on,
            rule_draft: None,
            rules_tab: Default::default(),
            rule_menu: None,
            adding_jev_key: false,
            jev_key,
            slash_selected: 0,
            slash_dismissed: None,
            slash_seen: String::new(),
            goal_continuations,
            goal_budget,
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

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// Takes a flagged call off the review queue, for good.
    pub fn mark_reviewed(
        &mut self,
        run: &RunId,
        call_id: &str,
        cx: &mut Context<Self>,
    ) {
        self.dismissed.insert((run.clone(), call_id.to_owned()));
        cx.emit(WorkspaceEvent::Reviewed {
            run: run.clone(),
            call_id: call_id.to_owned(),
        });
        cx.notify();
    }

    pub fn set_catalog(&mut self, catalog: Catalog, cx: &mut Context<Self>) {
        self.dismissed.extend(catalog.reviewed.iter().cloned());
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
            HostUpdate::RuleTrial(result) => self.set_rule_trial(result, cx),
            HostUpdate::QueryResult(result) => {
                self.set_query_result(result, cx)
            }
            HostUpdate::PullRequest { run, pr } => {
                self.set_pull_request(&run, *pr, cx)
            }
            HostUpdate::PullRequestState { run, state } => {
                self.set_pull_request_state(&run, state, cx)
            }
            HostUpdate::LandingPreview { run, preview } => {
                self.set_landing_preview(&run, preview, cx)
            }
            HostUpdate::Landed { run, landing } => {
                self.landed(&run, landing, cx)
            }
            HostUpdate::Dropped { run, result } => {
                self.dropped(&run, result, cx)
            }
            HostUpdate::Merged { run, result } => self.merged(&run, result, cx),
            HostUpdate::TauTurn { run, prompt } => {
                self.tau_turn(&run, prompt, cx)
            }
            HostUpdate::BranchCode { main, fork, code } => {
                self.set_branch_code(&main, &fork, code, cx)
            }
            HostUpdate::ResumeFailed(run) => self.resume_failed(&run, cx),
            HostUpdate::Titled { run, title } => self.retitle(&run, title, cx),
            HostUpdate::Repo(repo) => self.add_repo(repo, cx),
            HostUpdate::Setup(update) => self.update_setup(update, cx),
            HostUpdate::Snapshot { runs, catalog } => {
                self.set_catalog(*catalog, cx);
                self.replace_runs(runs, cx);
            }
        }
    }

    /// Echo what the host applies as [`HostUpdate`] events, or stop.
    pub fn set_mirrored(&mut self, mirrored: bool) {
        self.mirrored = mirrored;
    }

    /// What this workspace shows, for an interface that just connected.
    pub fn snapshot(&self) -> HostUpdate {
        HostUpdate::Snapshot {
            runs: self.runs.clone(),
            catalog: Box::new(self.catalog.clone()),
        }
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
        // task its parent handed it.
        if let RunEvent::RunStart {
            run,
            parent: Some(parent),
            agent,
        } = event
            && self.run(run).is_none()
            && let Some(view) = self.run(parent)
        {
            let task = view
                .items
                .iter()
                .rev()
                .find_map(|item| match item {
                    Item::Tool(card)
                        if card.tool == tau_vcs::delegate::NAME
                            && card.state == ToolState::Running =>
                    {
                        card.args.get("task")?.as_str().map(str::to_owned)
                    }
                    _ => None,
                })
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
        // A command's screen takes the output its card gained.
        if let RunEvent::ToolUpdate { run, call_id, .. }
        | RunEvent::ToolEnd { run, call_id, .. } = event
            && let Some(card) =
                self.run(run).and_then(|view| view.tool(call_id))
            && let ToolBody::Terminal(term) = &card.body
        {
            let term = term.clone();
            self.terms.get_mut().sync(run, call_id, &term, cx);
        }
        // A fork that finishes waits in its parent's chat, to land or
        // be dropped.
        if let RunEvent::RunEnd { run, .. } = event
            && let Some(Origin::Fork { from, .. }) =
                self.run(run).map(|view| view.origin.clone())
            && let Some(parent) =
                self.runs.iter_mut().find(|view| view.id == from)
            && !parent.items.iter().any(
                |item| matches!(item, Item::ForkReady { fork } if fork == run),
            )
        {
            parent.items.push(Item::ForkReady { fork: run.clone() });
        }
        // A sub-agent closes once its parent's call returns: landed, or
        // dropped.
        if let RunEvent::ToolEnd { run, call_id, .. } = event
            && let Some(view) = self.run(run)
            && view
                .tool(call_id)
                .is_some_and(|card| card.tool == tau_vcs::delegate::NAME)
        {
            let done: Vec<RunId> = view
                .children
                .iter()
                .filter(|child| {
                    child.kind == ChildKind::SubAgent && !child.status.is_live()
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
        match event {
            // The run has read what was queued.
            RunEvent::TurnStart { run, .. } => {
                self.queued.remove(run);
            }
            RunEvent::RunStart { run, .. } => {
                self.resuming.remove(run);
            }
            RunEvent::ToolEnd {
                run,
                call_id,
                is_error: false,
                ..
            } if self.proposes_landing(run, call_id) => {
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
        let run = self.current.clone();
        match plugin.screen? {
            PluginScreen::Plan => run.map(Route::Plan),
            PluginScreen::Memory => Some(Route::Memory {
                repo: self.selected_repo()?.to_owned(),
                note: None,
            }),
            PluginScreen::Constitution => Some(Route::Constitution {
                repo: self.selected_repo()?.to_owned(),
                rule: None,
            }),
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

    /// Opens or closes one file's hunks in a diff or show card.
    pub fn toggle_file(
        &mut self,
        run: &RunId,
        call_id: &str,
        path: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), call_id.to_owned(), path.to_owned());
        if !self.open_files.remove(&key) {
            self.open_files.insert(key);
        }
        cx.notify();
    }

    pub fn file_open(&self, run: &RunId, call_id: &str, path: &str) -> bool {
        self.open_files.contains(&(
            run.clone(),
            call_id.to_owned(),
            path.to_owned(),
        ))
    }

    /// Picks a change in a `vcs_log` card to show its detail, or puts
    /// it back when it is already the one picked.
    pub fn pick_change(
        &mut self,
        run: &RunId,
        call_id: &str,
        change_id: &str,
        cx: &mut Context<Self>,
    ) {
        let key = (run.clone(), call_id.to_owned());
        if self.picked_changes.get(&key).map(String::as_str) == Some(change_id)
        {
            self.picked_changes.remove(&key);
        } else {
            self.picked_changes.insert(key, change_id.to_owned());
        }
        cx.notify();
    }

    /// The change picked in a `vcs_log` card, by full change id.
    pub fn picked_change(&self, run: &RunId, call_id: &str) -> Option<&str> {
        self.picked_changes
            .get(&(run.clone(), call_id.to_owned()))
            .map(String::as_str)
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
        let (from, repo) =
            self.run(run).map_or_else(Default::default, |view| {
                (view.title.clone(), self.repo_of(view).to_owned())
            });
        // The note goes to the memory of the run's repository.
        if let Some(proposal) = proposal
            && let Some(repo) = self.catalog.repo_mut(&repo)
        {
            repo.memory.keep(&proposal, &from);
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
        let repo = self
            .run(run)
            .map_or(String::new(), |view| self.repo_of(view).to_owned());
        self.navigate(Route::Constitution { repo, rule }, cx);
    }

    /// Asks what landing `run` would do: on its parent for a fork, into
    /// trunk for a top-level run (ADR 0014).
    pub fn preview_landing(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::Previewing);
        let root = self
            .run(run)
            .is_some_and(|view| view.origin == Origin::Root);
        cx.emit(if root {
            WorkspaceEvent::PreviewMerge { run: run.clone() }
        } else {
            WorkspaceEvent::PreviewLanding { run: run.clone() }
        });
        cx.notify();
    }

    /// What merging `run` into trunk came to. Merged, its chat shows
    /// what went into trunk, and it closes; resolving, its card says so
    /// while its own turn resolves the conflicts.
    pub fn merged(
        &mut self,
        run: &RunId,
        result: Result<Merge, String>,
        cx: &mut Context<Self>,
    ) {
        match result {
            Ok(Merge::Merged { into, landing }) => {
                self.landings.remove(run);
                if let Some(view) =
                    self.runs.iter_mut().find(|view| &view.id == run)
                {
                    view.items.push(Item::Merged(MergedCard::from_record(
                        MergeRecord { into, landing },
                    )));
                }
                self.closed.insert(run.clone());
            }
            Ok(Merge::Resolving { conflicts }) => {
                self.landings
                    .insert(run.clone(), LandingState::Resolving(conflicts));
            }
            Err(error) => {
                self.landings
                    .insert(run.clone(), LandingState::Preview(Err(error)));
            }
        }
        cx.notify();
    }

    /// tau starts `run`'s next turn itself with `prompt`: resolving what a
    /// landing or a merge left in conflict (ADR 0014). It shows as tau's.
    pub fn tau_turn(
        &mut self,
        run: &RunId,
        prompt: String,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.runs.iter().position(|view| &view.id == run) else {
            return;
        };
        let mut view = self.runs.remove(at);
        self.closed.remove(run);
        self.resuming.insert(run.clone(), view.status.clone());
        view.items.push(Item::Tau(prompt));
        view.status = RunStatus::Planning;
        self.runs.insert(0, view);
        cx.notify();
    }

    /// The run proposed its landing with `vcs_land` (ADR 0014): once it
    /// stops, its landing card opens for the person to confirm.
    fn proposes_landing(&self, run: &RunId, call_id: &str) -> bool {
        self.run(run).is_some_and(|view| {
            view.items.iter().any(|item| {
                matches!(item, Item::Tool(card)
                    if card.call_id == call_id
                        && card.tool == "vcs_land"
                        && matches!(card.state, ToolState::Done { .. }))
            })
        })
    }

    pub fn set_landing_preview(
        &mut self,
        run: &RunId,
        preview: Result<Landing, String>,
        cx: &mut Context<Self>,
    ) {
        self.landings
            .insert(run.clone(), LandingState::Preview(preview));
        cx.notify();
    }

    /// Puts a landing preview away without landing.
    pub fn cancel_landing(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.remove(run);
        cx.notify();
    }

    /// Lands `run` on its parent, or merges it into trunk when it is a
    /// top-level run.
    pub fn land(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::Landing);
        let root = self
            .run(run)
            .is_some_and(|view| view.origin == Origin::Root);
        cx.emit(if root {
            WorkspaceEvent::Merge { run: run.clone() }
        } else {
            WorkspaceEvent::Land { run: run.clone() }
        });
        cx.notify();
    }

    /// What landing `run` came to. Landed, the parent's chat gets a card
    /// for it, the child's chat closes, and the parent opens.
    pub fn landed(
        &mut self,
        run: &RunId,
        landing: Result<Landing, String>,
        cx: &mut Context<Self>,
    ) {
        let landing = match landing {
            Ok(landing) => landing,
            Err(error) => {
                self.landings
                    .insert(run.clone(), LandingState::Preview(Err(error)));
                cx.notify();
                return;
            }
        };
        self.landings.remove(run);
        let Some(child) = self.run(run) else {
            return;
        };
        let parent = match &child.origin {
            Origin::Fork { from, .. } => from.clone(),
            Origin::SubAgent { parent } => parent.clone(),
            Origin::Root => return,
        };
        let card = LandedCard::from_record(LandingRecord {
            from: run.0.to_string(),
            title: child.title.clone(),
            landing,
        });
        if let Some(view) = self.runs.iter_mut().find(|view| view.id == parent)
        {
            view.items.retain(
                |item| !matches!(item, Item::ForkReady { fork } if fork == run),
            );
            view.items.push(Item::Landed(card));
        }
        self.close_run(run, cx);
        self.navigate(Route::Run(parent), cx);
    }

    /// Asks before dropping `run`, a child run.
    pub fn ask_drop(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::ConfirmDrop);
        cx.notify();
    }

    /// Drops `run`: its own changes are abandoned, and it closes.
    pub fn drop_child(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.landings.insert(run.clone(), LandingState::Dropping);
        cx.emit(WorkspaceEvent::DropChild { run: run.clone() });
        cx.notify();
    }

    /// What dropping `run` came to. Dropped, its chat closes and its
    /// parent opens.
    pub fn dropped(
        &mut self,
        run: &RunId,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = result {
            self.landings
                .insert(run.clone(), LandingState::Preview(Err(error)));
            cx.notify();
            return;
        }
        self.landings.remove(run);
        let parent = self.run(run).and_then(|child| match &child.origin {
            Origin::Fork { from, .. } => Some(from.clone()),
            Origin::SubAgent { parent } => Some(parent.clone()),
            Origin::Root => None,
        });
        if let Some(view) = parent.as_ref().and_then(|parent| {
            self.runs.iter_mut().find(|view| &view.id == parent)
        }) {
            view.items.retain(
                |item| !matches!(item, Item::ForkReady { fork } if fork == run),
            );
        }
        self.close_run(run, cx);
        if let Some(parent) = parent {
            self.navigate(Route::Run(parent), cx);
        }
    }

    pub fn landing(&self, run: &RunId) -> Option<&LandingState> {
        self.landings.get(run)
    }

    pub fn keep_branch(&mut self, run: &RunId, cx: &mut Context<Self>) {
        self.kept_branch = Some(run.clone());
        cx.emit(WorkspaceEvent::KeepBranch { run: run.clone() });
        cx.notify();
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

    pub(crate) fn last_fork_turn_of(run: &RunView) -> u32 {
        Self::last_fork_turn(run)
    }

    pub(crate) fn shows_inspector(&self) -> bool {
        self.inspector_shown
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
        let Some(run) = self.run(run) else {
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
            Some(run) if run.status.is_live() => {
                let run = run.id.clone();
                self.queued.insert(run.clone(), text.clone());
                cx.emit(WorkspaceEvent::Steer { run, text });
            }
            // A finished chat goes on.
            Some(run) => {
                let run = run.id.clone();
                self.resume_run(&run, text, cx);
            }
            _ => cx.emit(WorkspaceEvent::NewRun {
                prompt: text,
                model: self.next_model.clone(),
                repo: self.selected_repo().unwrap_or_default().to_owned(),
            }),
        }
        cx.notify();
    }

    /// Sends `text` to a finished run: it shows at once, the run moves to
    /// the top of the list, and the host starts it again.
    pub(crate) fn resume_run(
        &mut self,
        run: &RunId,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.runs.iter().position(|view| &view.id == run) else {
            return;
        };
        let mut view = self.runs.remove(at);
        // The model picked for it, or the one it was on.
        let model = self
            .run_models
            .remove(run)
            .unwrap_or_else(|| Self::model_of(&view));
        view.switch_model(&model.model, model.effort.label());
        // A message to a closed conversation opens it again.
        self.closed.remove(run);
        self.resuming.insert(run.clone(), view.status.clone());
        view.push_user(text.clone());
        view.status = RunStatus::Planning;
        self.runs.insert(0, view);
        self.follow = true;
        cx.emit(WorkspaceEvent::Resume {
            run: run.clone(),
            prompt: text,
            model,
        });
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
            if matches!(
                view.items.last(),
                Some(crate::view::Item::User(_) | crate::view::Item::Goal(_))
            ) {
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

    /// Moves the composer's caret a row, when it has focus and a row
    /// there.
    fn composer_row(
        &mut self,
        rows: i32,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.composer.read(cx).focus_handle(cx).is_focused(window)
            && self
                .composer
                .update(cx, |input, cx| input.move_vertical(rows, cx))
    }

    fn submit_from_button(&mut self, cx: &mut Context<Self>) {
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
        if self.run(run).is_some_and(|view| view.status.is_live()) {
            cx.emit(WorkspaceEvent::Cancel { run: run.clone() });
        }
        self.closed.insert(run.clone());
        self.hovered_run = None;
        cx.emit(WorkspaceEvent::CloseRun { run: run.clone() });
        if self.route.run() == Some(run) || self.current.as_ref() == Some(run) {
            // Show the next open conversation, or a new one.
            let next = self
                .runs
                .iter()
                .find(|view| {
                    !self.closed.contains(&view.id)
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
        let goal = self.setup_goal;
        match self.route {
            Route::Setup(SetupStep::GitHub | SetupStep::Token)
                if signed_in && goal == Some(SetupGoal::GitHub) =>
            {
                self.leave_setup(cx)
            }
            Route::Setup(SetupStep::GitHub | SetupStep::Token)
                if signed_in && goal == Some(SetupGoal::Repos) =>
            {
                self.navigate(Route::Setup(SetupStep::Repos), cx)
            }
            Route::Setup(SetupStep::GitHub | SetupStep::Token) if signed_in => {
                self.navigate(Route::Setup(SetupStep::Model), cx)
            }
            Route::Setup(SetupStep::Model)
                if connected && goal == Some(SetupGoal::Model) =>
            {
                self.leave_setup(cx)
            }
            // Onboarding stays to show the account and pick the default
            // model; "Continue" moves on.
            _ => cx.notify(),
        }
    }

    // Phones, on the computer.

    pub fn phones(&self) -> &Phones {
        &self.phones
    }

    pub fn set_phones(&mut self, phones: Phones, cx: &mut Context<Self>) {
        self.phones = phones;
        cx.notify();
    }

    pub fn ask_phones(
        &mut self,
        request: PhonesRequest,
        cx: &mut Context<Self>,
    ) {
        cx.emit(WorkspaceEvent::Phones(request));
    }

    // Pairing a phone.

    pub fn pairing(&self) -> &Pairing {
        &self.pairing
    }

    /// Replaces what pairing knows, as when the phone starts paired.
    pub fn set_pairing(&mut self, pairing: Pairing, cx: &mut Context<Self>) {
        self.pairing = pairing;
        cx.notify();
    }

    /// Opens pairing at `step`, with no way back to the runs until the
    /// phone reaches a computer.
    pub fn start_pairing(&mut self, step: PairStep, cx: &mut Context<Self>) {
        self.back_stack.clear();
        self.route = Route::Pair(step);
        self.entered(cx);
    }

    /// Records what the phone's remote learned: once paired, the phone
    /// is named; a computer that does not answer says so; one that
    /// answers again leads back to the runs.
    pub fn update_pairing(
        &mut self,
        update: PairingUpdate,
        cx: &mut Context<Self>,
    ) {
        let next = match &update {
            PairingUpdate::Paired(_) => Some(Route::Pair(PairStep::Paired)),
            PairingUpdate::Unreachable { .. } => {
                Some(Route::Pair(PairStep::Unreachable))
            }
            // Reached again: the runs, unless the phone just paired and
            // is being named.
            PairingUpdate::Connected(_) => {
                matches!(self.route, Route::Pair(PairStep::Unreachable))
                    .then_some(Route::Home)
            }
            PairingUpdate::Progress(_) => None,
        };
        self.pairing.update(update);
        match next {
            Some(route) => {
                self.back_stack.clear();
                self.route = route;
                self.entered(cx);
            }
            None => cx.notify(),
        }
    }

    /// Opens the camera to read a computer's pairing code.
    pub fn scan_pairing_code(&mut self, cx: &mut Context<Self>) {
        self.pairing.progress = Progress::Scanning;
        cx.emit(WorkspaceEvent::Pair(PairRequest::Scan));
        self.navigate(Route::Pair(PairStep::Scan), cx);
    }

    /// Pairs by typing the address and the code instead.
    pub fn type_address(&mut self, cx: &mut Context<Self>) {
        self.pairing.progress = Progress::Idle;
        self.navigate(Route::Pair(PairStep::Address), cx);
    }

    /// Connects to the typed address with the typed code.
    pub fn connect_typed(&mut self, cx: &mut Context<Self>) {
        if self.pairing.busy() {
            return;
        }
        let address =
            tau_remote::Address::typed(self.pair_address.read(cx).text());
        let secret =
            tau_remote::PairingSecret::typed(self.pair_code.read(cx).text());
        self.pairing.progress = match (address, secret) {
            (Ok(address), Ok(secret)) => {
                cx.emit(WorkspaceEvent::Pair(PairRequest::Typed {
                    address: address.clone(),
                    secret,
                }));
                Progress::Connecting { address }
            }
            (Err(error), _) | (_, Err(error)) => {
                Progress::Failed(error.to_string())
            }
        };
        cx.notify();
    }

    /// The person compared the certificate with the computer's, and it
    /// is the same: pair.
    pub fn trust_certificate(&mut self, cx: &mut Context<Self>) {
        if let Progress::Compare {
            address,
            fingerprint,
        } = &self.pairing.progress
        {
            cx.emit(WorkspaceEvent::Pair(PairRequest::Trust(*fingerprint)));
            self.pairing.progress = Progress::Pairing {
                address: address.clone(),
                fingerprint: *fingerprint,
            };
            cx.notify();
        }
    }

    /// The certificate is not the computer's: stop before sending the
    /// code.
    pub fn distrust_certificate(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::Pair(PairRequest::Cancel));
        self.pairing.progress = Progress::Failed(
            "The certificate is not your computer's, so tau stopped before \
             sending the code. Check the address, or scan the code instead."
                .into(),
        );
        cx.notify();
    }

    /// Names the phone, if a name was typed, and goes to the runs.
    pub fn open_tau(&mut self, cx: &mut Context<Self>) {
        let name = self.phone_name.read(cx).text().trim().to_owned();
        if !name.is_empty() {
            cx.emit(WorkspaceEvent::Pair(PairRequest::Name(name)));
        }
        self.back_stack.clear();
        self.route = Route::Home;
        self.entered(cx);
    }

    /// Connects to the paired computer again.
    pub fn retry_connection(&mut self, cx: &mut Context<Self>) {
        if self.pairing.busy() {
            return;
        }
        if let Some(computer) = &self.pairing.computer {
            self.pairing.progress = Progress::Connecting {
                address: computer.address.clone(),
            };
        }
        cx.emit(WorkspaceEvent::Pair(PairRequest::Retry));
        cx.notify();
    }

    /// The phone's own name, offered once paired.
    pub fn set_phone_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.phone_name
            .update(cx, |input, cx| input.set_text(name, cx));
    }

    /// Fills the pairing fields, as typing would: for tests.
    pub fn pair_fields_for_test(
        &mut self,
        address: &str,
        code: &str,
        cx: &mut Context<Self>,
    ) {
        self.pair_address
            .update(cx, |input, cx| input.set_text(address.to_owned(), cx));
        self.pair_code
            .update(cx, |input, cx| input.set_text(code.to_owned(), cx));
    }

    /// Asks for less motion: onboarding's loops, ripples and comets
    /// stop, and its transitions become plain fades.
    pub fn set_reduce_motion(&mut self, reduce: bool, cx: &mut Context<Self>) {
        self.reduce_motion = reduce;
        cx.notify();
    }

    pub fn reduce_motion(&self) -> bool {
        self.reduce_motion
    }

    /// What onboarding showed when last drawn, and what changed.
    pub fn setup_motion(&self) -> &crate::motion::SetupMotion {
        &self.setup_motion
    }

    /// Leaves the model step once signed in: on to the repositories, or
    /// to a new run when there are none to pick.
    pub fn continue_from_model(&mut self, cx: &mut Context<Self>) {
        if self.setup_goal == Some(SetupGoal::Model) {
            self.leave_setup(cx);
        } else if self.setup.repos.is_empty() {
            self.finish_setup(cx);
        } else {
            self.navigate(Route::Setup(SetupStep::Repos), cx);
        }
    }

    /// Makes model `id`, one of the account's, the one runs start with:
    /// the coder's default, as the picker sets it.
    pub fn pick_setup_model(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(option) = self.catalog.models.find(id).cloned() else {
            return;
        };
        let choice = ModelChoice {
            model: option.id.clone(),
            ..self.catalog.models.settings.default_for("coder")
        }
        .fitted();
        if let ModelAccess::Connected { label } = &mut self.setup.model {
            *label = format!("{} · ChatGPT plan", option.id);
        }
        self.set_default_model("coder", choice, cx);
    }

    /// Stops waiting for the browser, back to the start of the model
    /// step.
    pub fn cancel_chatgpt_sign_in(&mut self, cx: &mut Context<Self>) {
        self.setup.model = ModelAccess::None;
        self.chatgpt_callback
            .update(cx, |input, cx| input.clear(cx));
        cx.emit(WorkspaceEvent::ChatGptCancel);
        cx.notify();
    }

    /// Goes back to the screen an onboarding step was opened from.
    pub fn leave_setup(&mut self, cx: &mut Context<Self>) {
        let mut route = self.back_stack.pop().unwrap_or(Route::Home);
        while let Route::Setup(_) = route {
            route = self.back_stack.pop().unwrap_or(Route::Home);
        }
        self.route = route;
        self.entered(cx);
    }

    /// Opens GitHub's sign-in from the app, coming back once signed in.
    pub fn connect_github(&mut self, cx: &mut Context<Self>) {
        self.sign_in_github(cx);
        self.setup_goal = Some(SetupGoal::GitHub);
    }

    pub fn sign_out_github(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::GitHubSignOut);
    }

    /// Picks repositories to clone from GitHub, signing in first if
    /// needed, then comes back.
    pub fn pick_github_repos(&mut self, cx: &mut Context<Self>) {
        self.adding_repo = false;
        if self.setup.user().is_some() {
            self.navigate(Route::Setup(SetupStep::Repos), cx);
        } else {
            self.sign_in_github(cx);
        }
        self.setup_goal = Some(SetupGoal::Repos);
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

    /// Opens the model step of onboarding from the Models screen, to
    /// sign in to ChatGPT, coming back once done.
    pub fn connect_model(&mut self, cx: &mut Context<Self>) {
        self.setup.model = ModelAccess::None;
        self.navigate(Route::Setup(SetupStep::Model), cx);
        self.setup_goal = Some(SetupGoal::Model);
    }

    /// Fills the rule being written, as typing would: for tests.
    pub fn rule_text_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        self.rule_text
            .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    /// Fills "Another tool's field", as typing would: for tests.
    pub fn rule_on_for_test(&mut self, text: &str, cx: &mut Context<Self>) {
        self.rule_on
            .update(cx, |input, cx| input.set_text(text.to_owned(), cx));
    }

    pub fn remove_rule(
        &mut self,
        repo: &str,
        id: &str,
        cx: &mut Context<Self>,
    ) {
        cx.emit(WorkspaceEvent::RemoveRule {
            repo: repo.to_owned(),
            id: id.to_owned(),
        });
    }

    /// Asks for the TypeSafe key.
    pub fn ask_for_jev_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.adding_jev_key = true;
        self.jev_key.update(cx, |input, cx| input.clear(cx));
        self.jev_key.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    pub(crate) fn submit_jev_key(
        &mut self,
        key: String,
        cx: &mut Context<Self>,
    ) {
        let key = key.trim().to_owned();
        if key.is_empty() {
            return;
        }
        self.adding_jev_key = false;
        self.jev_key.update(cx, |input, cx| input.clear(cx));
        cx.emit(WorkspaceEvent::JevKey { key: Some(key) });
        cx.notify();
    }

    pub(crate) fn submit_jev_key_from_button(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let key = self.jev_key.read(cx).text().to_owned();
        self.submit_jev_key(key, cx);
    }

    pub fn forget_jev_key(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::JevKey { key: None });
    }

    /// The dialog that asks for the TypeSafe key.
    fn jev_key_view(
        &self,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let actions = div()
            .flex()
            .gap(sp(2.))
            .child(
                div()
                    .id("jev-key-cancel")
                    .child(ui::button("Cancel", ButtonKind::Secondary, t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.adding_jev_key = false;
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("jev-key-save")
                    .child(ui::button("Use this key", ButtonKind::Primary, t))
                    .on_click(cx.listener(|ws, _, _, cx| {
                        ws.submit_jev_key_from_button(cx)
                    })),
            );
        ui::modal(
            ui::icon(Icon::Key, IconSize::LARGE, t.muted),
            "TypeSafe key",
            "tau-constitution asks Jev, TypeSafe's model, whether calls break \
             a repository's rules. The key stays in tau's config directory, \
             readable only by you.",
            Some(
                ui::field(&self.jev_key, true, t)
                    .flex_shrink_0()
                    .into_any_element(),
            ),
            actions,
            t,
        )
    }

    pub fn sign_out(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::SignOut);
    }

    /// Signs in with ChatGPT in the browser: `account` again, or a new
    /// account with `None`; `consent` asks again for plan usage.
    pub fn sign_in_chatgpt(
        &mut self,
        account: Option<String>,
        consent: bool,
        cx: &mut Context<Self>,
    ) {
        self.setup.model = ModelAccess::SigningIn { url: None };
        self.chatgpt_callback
            .update(cx, |input, cx| input.clear(cx));
        cx.emit(WorkspaceEvent::ChatGptSignIn { account, consent });
        cx.notify();
    }

    /// Finishes the ChatGPT sign-in with a redirect URL pasted from the
    /// browser.
    pub(crate) fn submit_chatgpt_callback(
        &mut self,
        url: String,
        cx: &mut Context<Self>,
    ) {
        let url = url.trim().to_owned();
        if url.is_empty() {
            return;
        }
        self.chatgpt_callback
            .update(cx, |input, cx| input.clear(cx));
        cx.emit(WorkspaceEvent::ChatGptCallback { url });
        cx.notify();
    }

    pub(crate) fn submit_chatgpt_callback_from_button(
        &mut self,
        cx: &mut Context<Self>,
    ) {
        let url = self.chatgpt_callback.read(cx).text().to_owned();
        self.submit_chatgpt_callback(url, cx);
    }

    /// Signs in with this saved ChatGPT account from now on.
    pub fn switch_chatgpt(&mut self, account: &str, cx: &mut Context<Self>) {
        cx.emit(WorkspaceEvent::SwitchChatGpt {
            account: account.to_owned(),
        });
    }

    /// Asks again for plan usage on the active ChatGPT account, from the
    /// model setup, coming back once done.
    pub fn enable_plan_usage(&mut self, cx: &mut Context<Self>) {
        let account = self
            .catalog
            .models
            .access
            .active_account()
            .map(|account| account.id.clone());
        self.connect_model(cx);
        self.sign_in_chatgpt(account, true, cx);
    }

    /// Opens ChatGPT Settings → Usage, where the user reviews and limits
    /// what apps use of their plan.
    pub fn manage_usage(&mut self, cx: &mut Context<Self>) {
        cx.open_url(USAGE_SETTINGS_URL);
    }

    /// Dismisses the note on using the ChatGPT plan for good.
    pub fn dismiss_plan_notice(&mut self, cx: &mut Context<Self>) {
        self.catalog.models.settings.plan_notice_seen = true;
        cx.emit(WorkspaceEvent::SaveModelSettings(
            self.catalog.models.settings.clone(),
        ));
        cx.notify();
    }

    /// Whether the note on using the ChatGPT plan shows.
    pub fn shows_plan_notice(&self) -> bool {
        let models = &self.catalog.models;
        models.access.shows_plan_notice(&models.settings)
    }

    /// Whether runs use the ChatGPT plan, so the composer says so.
    pub fn uses_plan(&self) -> bool {
        self.catalog.models.access.chatgpt
    }

    /// A run stopped on the ChatGPT plan: says what to do next. A
    /// temporary refusal, which the run already retried, says nothing
    /// more than the run's error.
    pub fn show_plan_refusal(
        &mut self,
        refusal: &tau_ai::refusal::Refusal,
        cx: &mut Context<Self>,
    ) {
        if let Some(alert) = PlanAlert::of(refusal) {
            self.plan_alert = Some(alert);
            cx.notify();
        }
    }

    pub fn plan_alert(&self) -> Option<&PlanAlert> {
        self.plan_alert.as_ref()
    }

    /// Carries out a button of the plan alert, and closes it.
    pub fn plan_action(&mut self, action: PlanAction, cx: &mut Context<Self>) {
        self.plan_alert = None;
        let active = self
            .catalog
            .models
            .access
            .active_account()
            .map(|account| account.id.clone());
        match action {
            PlanAction::ManageUsage => self.manage_usage(cx),
            PlanAction::SignInAgain => {
                self.connect_model(cx);
                self.sign_in_chatgpt(active, false, cx);
            }
            PlanAction::EnablePlanUsage => self.enable_plan_usage(cx),
            PlanAction::Close => {}
        }
        cx.notify();
    }

    /// The plan alert, over the app.
    fn plan_alert_view(
        &self,
        alert: &PlanAlert,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (secondary, other) = alert.secondary();
        let actions = div()
            .flex()
            .gap(sp(2.))
            .child(
                div()
                    .id("plan-alert-secondary")
                    .child(ui::button(secondary, ButtonKind::Secondary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.plan_action(other, cx)
                    })),
            )
            .children(alert.primary().map(|(label, action)| {
                div()
                    .id("plan-alert-primary")
                    .child(ui::button(label, ButtonKind::Primary, t))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        ws.plan_action(action, cx)
                    }))
            }));
        let glyph = match alert {
            PlanAlert::UsageLimit => {
                ui::icon(Icon::Warning, IconSize::LARGE, t.accent)
            }
            _ => ui::icon(Icon::Chat, IconSize::LARGE, t.muted),
        };
        ui::modal(glyph, alert.title(), alert.message(), None, actions, t)
    }

    /// The note shown once after signing in with plan usage: what the
    /// plan pays for, where to manage it, and "Got it".
    fn plan_notice(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .items_start()
            .gap(sp(2.5))
            .px(sp(3.5))
            .py(sp(3.))
            .rounded(radius::BOX)
            .border_1()
            .border_color(t.blue_border)
            .bg(t.blue_soft)
            .child(div().mt(sp(0.25)).child(ui::icon(
                Icon::Chat,
                IconSize::MEDIUM,
                t.blue,
            )))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .flex_col()
                    .gap(sp(1.))
                    .child(ui::text(
                        plan_usage::NOTICE_TITLE,
                        Type::SMALL,
                        t.text,
                    ))
                    .child(ui::text(
                        plan_usage::NOTICE_BODY,
                        Type::CAPTION,
                        t.muted,
                    ))
                    .child(
                        div()
                            .id("plan-notice-manage")
                            .child(ui::text_link(
                                plan_usage::MANAGE_USAGE,
                                Type::CAPTION,
                                t,
                            ))
                            .on_click(
                                cx.listener(|ws, _, _, cx| ws.manage_usage(cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .id("plan-notice-dismiss")
                    .child(ui::button("Got it", ButtonKind::Secondary, t))
                    .on_click(
                        cx.listener(|ws, _, _, cx| ws.dismiss_plan_notice(cx)),
                    ),
            )
    }

    /// Under the composer while runs use the plan: `Using ChatGPT plan ·
    /// Manage usage`.
    fn plan_indicator(&self, t: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        div()
            .flex()
            .items_center()
            .gap(sp(1.5))
            .typeset(Type::CAPTION)
            .text_color(t.muted)
            .child(ui::icon(Icon::Chat, IconSize::SMALL, t.green))
            .child(plan_usage::INDICATOR)
            .child(ui::text("·", Type::CAPTION, t.dim))
            .child(
                div()
                    .id("plan-manage-usage")
                    .child(ui::text_link(
                        plan_usage::MANAGE_USAGE,
                        Type::CAPTION,
                        t,
                    ))
                    .on_click(cx.listener(|ws, _, _, cx| ws.manage_usage(cx))),
            )
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
        // From the app, the clones show up in the sidebar; from
        // onboarding, the first run comes next.
        if self.setup_goal == Some(SetupGoal::Repos) {
            self.leave_setup(cx);
        } else {
            self.navigate(Route::Setup(SetupStep::Ready), cx);
        }
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
        cx.emit(WorkspaceEvent::NewRun {
            prompt: task,
            model: self.next_model.clone(),
            repo: self.selected_repo().unwrap_or_default().to_owned(),
        });
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
        if self.rule_menu.take().is_some() {
            cx.notify();
        } else if self.rule_draft.is_some() {
            self.close_rule_editor(cx);
        } else if self.slash_dismiss(cx) {
        } else if self.dialog.is_some() {
            self.dismiss_alert(cx);
        } else if self.plan_alert.take().is_some() {
            cx.notify();
        } else if self.searching {
            self.close_search(cx);
        } else if self.adding_repo {
            self.cancel_add_repo(cx);
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
    ) -> AnyElement {
        if self
            .current()
            .filter(|_| self.route != Route::NewRun)
            .is_none()
        {
            return div()
                .id("transcript")
                .flex_1()
                .min_h(px(0.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(sp(3.))
                .text_color(t.muted)
                .child(ui::logo(t, 40.))
                .child("A new run. Describe the task below.")
                .into_any_element();
        }
        let theme = t.clone();
        gpui::list(
            self.transcript.clone(),
            cx.processor(move |ws, index: usize, window, cx| {
                let Some(run) =
                    ws.current().filter(|_| ws.route != Route::NewRun)
                else {
                    return div().into_any_element();
                };
                transcript::item(ws, run, index, &theme, compact, window, cx)
            }),
        )
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    }

    /// How wide the transcript was last laid out; `None` before it was.
    pub(crate) fn transcript_width(&self) -> Option<gpui::Pixels> {
        self.transcript_width
    }

    /// Takes the focus back from an overlay's field once the overlay is
    /// gone. Left on a field no longer drawn, keys would reach nothing,
    /// not even ctrl+k to open search again.
    fn release_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let stranded = (!self.searching
            && self.search.read(cx).focus_handle(cx).is_focused(window))
            || (self.picker.is_none()
                && self
                    .model_search
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window));
        if stranded {
            self.focus.focus(window, cx);
        }
    }

    /// Tells the transcript list what changed in the open run since the
    /// last frame: items added, or another run opened. An item in view is
    /// laid out again every frame, so one that grows as it streams needs
    /// no telling.
    fn sync_transcript(&mut self) {
        let width = self.transcript.viewport_bounds().size.width;
        self.transcript_width = (width > px(0.)).then_some(width);
        // One more row, past the items, for an open landing's card.
        let now =
            self.current()
                .filter(|_| self.route != Route::NewRun)
                .map(|run| {
                    let card = usize::from(self.landings.contains_key(&run.id));
                    (run.id.clone(), run.items.len() + card)
                });
        match (&now, &self.listed) {
            (Some((run, count)), Some((listed, before))) if run == listed => {
                if count > before {
                    // The last known item may have changed too.
                    let from = before.saturating_sub(1);
                    self.transcript.splice(from..*before, count - from);
                } else if count < before {
                    self.transcript.reset(*count);
                }
            }
            (Some((_, count)), _) => self.transcript.reset(*count),
            (None, _) => {}
        }
        self.listed = now;
        if self.follow {
            self.transcript.scroll_to(ListOffset {
                item_ix: usize::MAX,
                offset_in_item: px(0.),
            });
        }
    }

    /// Land on the parent, or merge into main: in the run's bar, for a
    /// finished fork or top-level run (ADR 0014). It opens the landing
    /// card at the end of the chat.
    fn land_button(
        &self,
        run: &RunView,
        t: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        let target = ui::landing::target(self, run)?;
        if run.status.is_live() || self.closed.contains(&run.id) {
            return None;
        }
        let label = if run.origin == Origin::Root {
            format!("Merge into {target}")
        } else {
            format!("Land on {target}")
        };
        let id = run.id.clone();
        Some(
            div()
                .id("land")
                .child(ui::button(label, ButtonKind::Primary, t))
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.follow = true;
                    ws.preview_landing(&id, cx)
                })),
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
                    .map(|fork| (run.id.clone(), fork.id.clone()))
                    // A fork compares with the run it came from.
                    .or_else(|| match &run.origin {
                        Origin::Fork { from, .. } => {
                            Some((from.clone(), run.id.clone()))
                        }
                        _ => None,
                    })
                    .map(|(main, fork)| {
                        let route = Route::Compare { main, fork };
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
            .children(self.land_button(run, t, cx))
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
        // On New Run the message starts a run, whatever the open run does.
        let live = self.route != Route::NewRun && self.is_live();
        let continues = self.continues_chat();
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
            // As tall as the field beside it, with the field's corners.
            send.child(
                ui::big_button(
                    if self.forking.is_some() {
                        "Fork"
                    } else if live {
                        "Steer"
                    } else if continues {
                        "Send"
                    } else {
                        "Start"
                    },
                    Some(if self.forking.is_some() {
                        Icon::Fork
                    } else {
                        Icon::Send
                    }),
                    ButtonKind::Primary,
                    t,
                )
                .h(control::LARGE)
                .rounded(radius::LARGE)
                .shadow_sm(),
            )
        };

        div()
            .relative()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .children(self.slash_popover(compact, t, cx))
            .px(sp(if compact { 3. } else { 6. }))
            .pt(sp(if compact { 2.5 } else { 0. }))
            .pb(sp(if compact { 4.5 } else { 4. }))
            .when(compact, |bar| bar.border_t_1().border_color(t.border))
            .when_some(self.composer_target().filter(|_| compact), |bar, target| {
                bar.child(div().flex().child(self.model_chip(target, t, cx)))
            })
            .when(self.shows_plan_notice(), |bar| {
                bar.child(self.plan_notice(t, cx))
            })
            .when_some(self.fork_banner(t, cx), |bar, banner| bar.child(banner))
            .when(!self.attachments.is_empty(), |bar| {
                bar.child(
                    div().flex().flex_wrap().gap(sp(1.5)).children(
                        self.attachments.iter().enumerate().map(|(n, file)| {
                            div()
                                .flex()
                                .items_center()
                                .gap(sp(1.5))
                                .px(sp(2.))
                                .py(sp(1.))
                                .rounded(radius::CONTROL)
                                .bg(t.raised)
                                .typeset(Type::CAPTION)
                                .child(ui::icon(Icon::Paperclip, IconSize::SMALL, t.muted))
                                .child(file.name.clone())
                                .child(
                                    div()
                                        .id(("detach", n))
                                        .cursor_pointer()
                                        .child(ui::icon(Icon::Close, IconSize::SMALL, t.dim))
                                        .on_click(cx.listener(move |ws, _, _, cx| {
                                            ws.remove_attachment(n, cx)
                                        })),
                                )
                        }),
                    ),
                )
            })
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
                    // A taller field keeps the buttons by its last line.
                    .items_end()
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
                            .items_end()
                            .gap(sp(2.5))
                            .min_h(control::LARGE)
                            .px(sp(3.5))
                            .py(sp(2.))
                            .border_1()
                            .border_color(t.border_strong)
                            .rounded(if compact { radius::FULL } else { radius::LARGE })
                            .bg(t.panel)
                            .when(!compact, |field| {
                                field.child(
                                    // As tall as the chip, so it lines up
                                    // with the first line of text.
                                    div()
                                        .id("attach")
                                        .h(control::SMALL)
                                        .flex()
                                        .items_center()
                                        .cursor_pointer()
                                        .child(ui::icon(Icon::Paperclip, IconSize::BASE, t.muted))
                                        .on_click(cx.listener(|ws, _, _, cx| {
                                            ws.pick_attachments(cx)
                                        })),
                                )
                            })
                            // Its first line level with the chip beside it.
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .py(sp(0.75))
                                    .child(self.composer.clone()),
                            )
                            .when_some(
                                self.composer_target().filter(|_| !compact),
                                |field, target| {
                                    field.child(self.model_chip(target, t, cx))
                                },
                            ),
                    )
                    .child(send),
            )
            .when(self.uses_plan(), |bar| {
                bar.child(self.plan_indicator(t, cx))
            })
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
                .child(div().text_color(t.text_soft).child("on"))
                .child(self.model_chip(PickerTarget::Fork, t, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_color(t.muted)
                        .child(
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
        let run = self.current();
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
                    .items_center()
                    .px(sp(4.))
                    .border_b_1()
                    .border_color(t.border)
                    .children(run.map(|run| inspector::header(run, t))),
            )
            .child(
                div()
                    .id("inspector")
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .p(sp(4.))
                    .children(
                        run.map(|run| inspector::content(self, run, t, cx)),
                    ),
            )
            .children(run.map(|run| inspector::events(self, run, t, cx)))
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
            .when(!compact && self.route != Route::NewRun, |screen| {
                screen.children(
                    self.current().and_then(|run| self.goal_banner(run, t, cx)),
                )
            })
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
            Route::Memory { repo, note } => screens::memory::render(
                self,
                repo,
                note.as_deref(),
                compact,
                t,
                cx,
            ),
            Route::Constitution { repo, rule } => {
                screens::constitution::render(
                    self,
                    repo,
                    rule.as_deref(),
                    compact,
                    t,
                    cx,
                )
            }
            Route::Ledger(run) => {
                screens::ledger::render(self, run, compact, t, cx)
            }
            Route::Setup(_) | Route::Pair(_) | Route::PullRequest(_) => {
                self.focused(compact, t, cx)
            }
            Route::Models => screens::models::render(self, compact, t, cx),
            Route::Phones => screens::phones::render(self, compact, t, cx),
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
                            .w(sidebar_width(wide))
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
            Route::Pair(step) => {
                screens::pairing::render(self, *step, compact, t, cx)
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
                .children(self.phone_goal_bar(run, t, cx))
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
        let live = run.status.is_live();
        let done = self.catalog.pull_requests
            && run.status == RunStatus::Finished(StopReason::Stop);
        let id = run.id.clone();
        div()
            .absolute()
            .inset_0()
            .occlude()
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
                    .child(inspector::header(run, t))
                    .child(
                        div()
                            .id("sheet-body")
                            .flex_1()
                            .min_h(px(0.))
                            .overflow_y_scroll()
                            .child(inspector::content(self, run, t, cx)),
                    )
                    .child(inspector::events(self, run, t, cx))
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

/// The desktop sidebar's width: wider on a wide window.
fn sidebar_width(wide: bool) -> gpui::Pixels {
    px(if wide { 264. } else { 232. })
}

impl Workspace {
    /// How wide a screen beside the sidebar is drawn: the window less
    /// the sidebar, or the whole window on a phone.
    pub fn screen_width(&self) -> gpui::Pixels {
        if self.phone_preview || self.width < PHONE_MAX {
            self.width
        } else {
            self.width - sidebar_width(self.width >= NARROW_MAX)
        }
    }
}

impl Render for Workspace {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.sync_transcript();
        self.release_focus(window, cx);
        if let Route::Setup(step) = self.route {
            screens::setup::observe(self, step, window, cx);
        }
        let t = theme(cx).clone();
        let width = match self.frame {
            Some((width, _)) => px(width),
            None => window.viewport_size().width,
        };
        let phone = self.phone_preview || width < PHONE_MAX;
        self.width = if self.phone_preview { px(390.) } else { width };
        self.inspector_shown = !phone
            && width >= NARROW_MAX
            && matches!(self.route, Route::Home | Route::Run(_));
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
            .when_some(self.model_overlay(phone, &t, cx), |body, overlay| {
                body.child(overlay)
            })
            .when(self.adding_repo, |body| {
                body.child(self.add_repo_view(&t, cx))
            })
            .when(self.searching, |body| body.child(self.search_view(&t, cx)))
            .when(self.adding_jev_key, |body| {
                body.child(self.jev_key_view(&t, cx))
            })
            .when_some(self.plan_alert.clone(), |body, alert| {
                body.child(self.plan_alert_view(&alert, &t, cx))
            })
            .when_some(self.dialog.clone(), |body, dialog| {
                body.child(self.dialog_view(dialog, &t, cx))
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
            // Up and down move through the slash menu, else between the
            // composer's lines.
            .on_action(cx.listener(|ws, _: &SlashUp, window, cx| {
                if !ws.slash_move(-1, cx) && !ws.composer_row(-1, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &SlashDown, window, cx| {
                if !ws.slash_move(1, cx) && !ws.composer_row(1, window, cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &SlashComplete, _, cx| {
                if !ws.slash_complete(cx) {
                    cx.propagate();
                }
            }))
            .on_action(cx.listener(|ws, _: &NewRun, window, cx| {
                ws.start_new_run(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &Search, window, cx| {
                ws.open_search(window, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowRuns, _, cx| {
                ws.switch_tab(route::Tab::Runs, cx)
            }))
            .on_action(cx.listener(|ws, _: &ShowMemory, _, cx| {
                if let Some(repo) = ws.selected_repo().map(str::to_owned) {
                    ws.open_memory(&repo, cx);
                }
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

/// Where landing a child run stands.
#[derive(Debug, Clone, PartialEq)]
pub enum LandingState {
    /// Waiting for the host's preview.
    Previewing,
    /// What landing would do, or why it cannot.
    Preview(Result<Landing, String>),
    /// Landing now.
    Landing,
    /// Asking before dropping the child.
    ConfirmDrop,
    /// Merging into trunk waits on the run's own turn resolving these
    /// conflicts.
    Resolving(Vec<String>),
    /// Dropping now.
    Dropping,
}
