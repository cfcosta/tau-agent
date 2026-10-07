//! One tau, its computer's window and phones (decision 0013), against
//! what a person who used any of them must see on all of them.
//!
//! The computer runs the real host, with a model that answers only when
//! the test says so, so runs stay live as long as the test wants. The
//! phones reach it through the real feed and the real encoding, in
//! process: what the network would do is the test's to pick, message
//! by message, including dropping a phone and bringing it back.
//!
//! Whenever every message has arrived, every phone shows what the
//! computer shows ([`Workspace::synced`]), and the computer shows what
//! the person asked for, from whichever device they asked.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    rc::Rc,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::{FutureExt as _, StreamExt as _, future::BoxFuture, stream};
use gpui::{Entity, TestAppContext, VisualTestContext};
use hegel::{HealthCheck, TestCase, generators as gs};
use serde_json::Value;
use tau_agent::{agent::Agent, tool::RunId};
use tau_ai::{
    llm::{EventStream, Llm, LlmError, LlmSession},
    message::{Message, Timestamp},
    responses::request::Settings,
};
use tau_remote::{
    Down,
    Up,
    feed::{Feed, Joined},
    outbox::{self, Outbox},
};
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
    phone_server,
};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    models::{Effort, ModelChoice},
    phones::PhoneUp,
    remote,
    route::Route,
    update::HostUpdate,
    view::{Item, Origin, RunView},
    workspace::Synced,
};
use tau_vcs_host::{Identity, Project};
use tokio::sync::oneshot;

/// The models people show, hide and pick: the plan's.
fn models() -> Vec<String> {
    let offered = tau_ai::model::plan_models();
    offered
        .iter()
        .take(3)
        .map(|model| model.id.clone())
        .collect()
}

/// The repositories the host lists.
const REPOS: [&str; 2] = ["repo", "other"];
/// How long the host has to settle before the test fails.
const SETTLE: Duration = Duration::from_secs(30);

/// What the model answers a request with.
#[derive(Debug, Clone, PartialEq)]
enum Reply {
    /// It is done: the turn ends, and so does the run.
    Text,
    /// It writes `content` to `path`: the chat has changes to land.
    Write { path: String, content: String },
    /// It hands `task` to a sub-agent, which only a main chat can, and
    /// goes on without waiting for it.
    Delegate { task: String },
}

/// A model that answers each request only when [`Gate::answer`] says,
/// and with what it says.
#[derive(Clone, Default)]
struct Gate {
    model: ScriptedModel,
    waiting: Arc<Mutex<Vec<oneshot::Sender<Reply>>>>,
    /// Sessions open: one per run going on.
    sessions: Arc<AtomicUsize>,
}

impl Gate {
    /// The requests still waiting: a cancelled run's went away.
    fn waiting(&self) -> usize {
        let mut waiting = self.waiting.lock().unwrap();
        waiting.retain(|answer| !answer.is_closed());
        waiting.len()
    }

    /// Answers the `n`th waiting request with `reply`.
    fn answer(&self, n: usize, reply: Reply) {
        let answer = self.waiting.lock().unwrap().remove(n);
        let _ = answer.send(reply);
    }
}

impl Llm for Gate {
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        // A plugin's side request, such as a commit message, has no
        // tools; a run's turns always do. Side requests are answered at
        // once: the test steers runs, not what plugins ask on the side.
        let side = settings.tools.is_empty();
        let (gate, open) = (self.clone(), self.model.open(settings));
        if !side {
            gate.sessions.fetch_add(1, Ordering::SeqCst);
        }
        async move {
            let inner = Arc::new(Mutex::new(open.await?));
            let settings = inner.lock().unwrap().settings().clone();
            let session = GatedSession {
                gate,
                inner,
                settings,
                side,
            };
            Ok(Box::new(session) as Box<dyn LlmSession>)
        }
        .boxed()
    }
}

struct GatedSession {
    gate: Gate,
    /// The scripted session, answered once the test says how.
    inner: Arc<Mutex<Box<dyn LlmSession>>>,
    /// Its settings, as the inner session has them.
    settings: Settings,
    /// A plugin's side request, answered at once.
    side: bool,
}

impl Drop for GatedSession {
    fn drop(&mut self) {
        if !self.side {
            self.gate.sessions.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

impl LlmSession for GatedSession {
    fn settings(&self) -> &Settings {
        &self.settings
    }

    fn set_reasoning(
        &mut self,
        effort: Option<tau_ai::responses::request::ReasoningEffort>,
    ) {
        self.settings.reasoning = effort;
        self.inner.lock().unwrap().set_reasoning(effort);
    }

    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream {
        if self.side {
            let _ = self.gate.model.clone().turn(|turn| turn.text("Notes."));
            return self.inner.lock().unwrap().respond(transcript, timestamp);
        }
        let (answer, answered) = oneshot::channel();
        self.gate.waiting.lock().unwrap().push(answer);
        let (model, inner, transcript) = (
            self.gate.model.clone(),
            self.inner.clone(),
            transcript.to_vec(),
        );
        // The turn is scripted once the test answers, as it answers.
        let reply = async move {
            let reply = answered.await.unwrap_or(Reply::Text);
            let _ = model.turn(|turn| match reply {
                Reply::Text => turn.text("Done."),
                Reply::Write { path, content } => turn.tool_call(
                    "write",
                    serde_json::json!({ "path": path, "content": content }),
                ),
                Reply::Delegate { task } => {
                    turn.tool_call("spawn", serde_json::json!({ "task": task }))
                }
            });
            inner.lock().unwrap().respond(&transcript, timestamp)
        };
        stream::once(reply).flatten().boxed()
    }
}

/// A phone's end of the network.
struct Link {
    /// Its connection, while it has one.
    conn: Option<u32>,
    /// What the computer sent that has not arrived, answers included.
    down: VecDeque<Down>,
    /// What the phone sent on its connection that has not arrived.
    up: VecDeque<Up>,
    /// What the person asked on the phone that the computer has not
    /// taken, as `remote` keeps it.
    outbox: Outbox,
    /// The last request the computer took from it, as `phone_server`
    /// keeps it.
    taken: Option<u64>,
    /// The last message's number it applied.
    last: Option<u64>,
    /// It needs a snapshot the computer has not sent.
    waiting: bool,
}

/// The network between the computer and its phones.
struct Wire {
    feed: Feed<u32>,
    links: Vec<Link>,
    next_conn: u32,
}

impl Wire {
    fn broadcast(&mut self, body: Value) {
        let (down, to) = self.feed.broadcast(body);
        for link in &mut self.links {
            if link.conn.is_some_and(|conn| to.contains(&conn)) {
                link.down.push_back(down.clone());
            }
        }
    }

    fn connect(&mut self, phone: usize) {
        let conn = self.next_conn;
        self.next_conn += 1;
        let link = &mut self.links[phone];
        link.conn = Some(conn);
        match self.feed.join(conn, link.last) {
            Joined::Replay(missed) => link.down.extend(missed),
            Joined::NeedSnapshot => link.waiting = true,
        }
        // What the computer did not take yet goes again, in order.
        let pending: Vec<Up> = link.outbox.pending().collect();
        link.up.extend(pending);
    }

    fn settled(&self) -> bool {
        self.links.iter().all(|link| {
            link.conn.is_some()
                && link.down.is_empty()
                && link.up.is_empty()
                && link.outbox.is_empty()
                && !link.waiting
        })
    }
}

struct Device {
    workspace: Entity<Workspace>,
    cx: VisualTestContext,
}

impl Device {
    fn synced(&mut self) -> Synced {
        self.workspace.read_with(&self.cx, |ws, _| ws.synced())
    }

    fn runs(&mut self) -> Vec<RunView> {
        self.workspace
            .read_with(&self.cx, |ws, _| ws.runs().to_vec())
    }
}

/// What the person asked for, in the order the host got it from any
/// device: what the computer must show once everything arrived.
#[derive(Debug, Default)]
struct Asked {
    /// Each message, its run (none for a new chat), and whether it may
    /// not show: the run was stopped after it, and a steer it had not
    /// read goes with it, or the host refused the new chat and said so.
    said: Vec<(Option<RunId>, String, bool)>,
    /// Chats closed and not written to since.
    closed: HashSet<RunId>,
    /// Chats stopped and not written to since.
    cancelled: HashSet<RunId>,
    /// Each chat's last goal set, through tau-goal's records.
    goals: HashMap<RunId, String>,
    /// Each model hidden or shown last.
    hidden: HashMap<String, bool>,
    /// The model coder runs on last picked.
    default: Option<ModelChoice>,
    /// Messages the host got more than once.
    twice: Vec<String>,
    /// Chats asked to land or be dropped, and not written to or refused
    /// since: each lands, waits in its main chat's queue, or is dropped.
    ending: HashSet<RunId>,
    /// The branch kept last.
    kept: Option<RunId>,
    /// Repositories hidden.
    hidden_repos: HashSet<String>,
    /// Chats ever asked to land or be dropped, and the messages that came
    /// for them after: the host refuses those once the chat ended.
    asked_to_end: HashSet<RunId>,
    /// Chats the host said landed or were dropped, asked for or not: a
    /// sub-agent nobody waits for lands as it returns (ADR 0026). A
    /// message the host gets for one after is late too: a phone's,
    /// typed while the chat still ran.
    ended: HashSet<RunId>,
    late: HashSet<String>,
    /// What the host said went wrong, for a failure to show.
    alerts: Vec<String>,
}

impl Asked {
    /// The host got `event`.
    fn arrived(&mut self, event: &WorkspaceEvent) {
        match event {
            WorkspaceEvent::NewRun { prompt, repo, .. } => {
                self.said_once(prompt);
                // A repository hidden before cannot take a new chat, nor
                // can none, once two devices hid one each; the host says
                // so.
                let refused =
                    repo.is_empty() || self.hidden_repos.contains(repo);
                self.said.push((None, prompt.clone(), refused))
            }
            WorkspaceEvent::Fork { prompt, .. } => {
                self.said_once(prompt);
                self.said.push((None, prompt.clone(), false))
            }
            WorkspaceEvent::Land { run }
            | WorkspaceEvent::DropChild { run } => {
                self.ending.insert(run.clone());
                self.asked_to_end.insert(run.clone());
            }
            WorkspaceEvent::KeepBranch { run } => self.kept = Some(run.clone()),
            WorkspaceEvent::HideRepo { repo } => {
                self.hidden_repos.insert(repo.clone());
            }
            WorkspaceEvent::Say { run, text, .. } => {
                self.said_once(text);
                if self.asked_to_end.contains(run) || self.ended.contains(run) {
                    self.late.insert(text.clone());
                }
                self.said.push((Some(run.clone()), text.clone(), false));
                self.ending.remove(run);
                // It may open the chat again, or start it again.
                self.closed.remove(run);
                self.cancelled.remove(run);
            }
            WorkspaceEvent::CloseRun { run } => {
                self.closed.insert(run.clone());
            }
            WorkspaceEvent::PluginRecord { run, plugin, body }
                if plugin == tau_goal::NAME =>
            {
                if let Some(tau_goal::Record::Set { goal, .. }) =
                    tau_goal::Record::parse(body)
                {
                    self.goals.insert(run.clone(), goal);
                }
            }
            WorkspaceEvent::HideModel { id, hidden } => {
                self.hidden.insert(id.clone(), *hidden);
            }
            WorkspaceEvent::SetDefaultModel { agent, choice }
                if agent == "coder" =>
            {
                self.default = Some(choice.clone());
            }
            WorkspaceEvent::Cancel { run } => {
                for (said, _, stopped) in &mut self.said {
                    *stopped |= said.as_ref() == Some(run);
                }
                self.cancelled.insert(run.clone());
            }
            _ => {}
        }
    }

    /// The host answered: a landing or a drop it refused, it said so.
    fn answered(&mut self, update: &HostUpdate) {
        match update {
            HostUpdate::Alert { title, message } => {
                // The host answers a new chat as it takes the request: a
                // refusal is of the last one, such as one from a main
                // chat left with conflicts (ADR 0024).
                if title == "Could not start the run"
                    || title == "Could not fork the run"
                {
                    let last = self
                        .said
                        .iter_mut()
                        .rev()
                        .find(|(run, ..)| run.is_none());
                    if let Some((_, _, refused)) = last {
                        *refused = true;
                    }
                }
                self.alerts.push(format!("{title}: {message}"))
            }
            // A turn tau starts, reporting sub-agents or resolving a
            // landing, is a new turn: the one the person stopped stays
            // stopped.
            HostUpdate::TauTurn { run, .. } => {
                self.cancelled.remove(run);
            }
            HostUpdate::Landed {
                run,
                landing: Err(_),
            }
            | HostUpdate::Dropped {
                run,
                result: Err(_),
            } => {
                self.ending.remove(run);
            }
            HostUpdate::Landed {
                run,
                landing: Ok(_),
            }
            | HostUpdate::Dropped {
                run,
                result: Ok(()),
            } => {
                self.ended.insert(run.clone());
            }
            _ => {}
        }
    }

    /// Every message is told apart: one the host got before came twice.
    fn said_once(&mut self, text: &str) {
        if self.said.iter().any(|(_, said, _)| said == text) {
            self.twice.push(text.to_owned());
        }
    }

    /// Fails unless `computer` shows what was asked, once.
    fn shown_by(&self, computer: &Synced) {
        assert!(self.twice.is_empty(), "taken twice: {:?}", self.twice);
        let users = |run: &RunView| -> Vec<String> {
            run.items
                .iter()
                .filter_map(|item| match item {
                    Item::User(text) => Some(text.clone()),
                    _ => None,
                })
                .collect()
        };
        let shows = |run: Option<&RunId>, text: &String| {
            let in_transcript = computer
                .runs
                .iter()
                .filter(|view| run.is_none_or(|run| &view.id == run))
                .any(|view| users(view).contains(text));
            // A steer the run has not read yet shows as queued.
            let queued = run.and_then(|run| computer.queued.get(run));
            // A sub-agent that ended before reading it handed it to its
            // main chat, quoted in a message there (ADR 0026).
            let quoted = format!("> {text}");
            let forwarded = run
                .and_then(|run| {
                    computer.runs.iter().find(|view| &view.id == run)
                })
                .and_then(|view| match &view.origin {
                    Origin::SubAgent { parent } => Some(parent),
                    _ => None,
                })
                .and_then(|parent| {
                    computer.runs.iter().find(|view| &view.id == parent)
                })
                .is_some_and(|main| {
                    main.items.iter().any(|item| match item {
                        Item::User(said) | Item::Tau(said) => {
                            said.contains(&quoted)
                        }
                        _ => false,
                    })
                });
            in_transcript
                || forwarded
                || queued.is_some_and(|texts| texts.contains(text))
        };
        // A chat that landed or was dropped takes no more messages; the
        // host says so.
        let ended = |run: Option<&RunId>| {
            computer
                .runs
                .iter()
                .any(|view| Some(&view.id) == run && view.ending.is_some())
        };
        for (run, text, stopped) in &self.said {
            let refused = self.late.contains(text) && ended(run.as_ref());
            assert!(
                *stopped || refused || shows(run.as_ref(), text),
                "the computer does not show {text:?} to {run:?}; it said \
                 {:?}; it shows {:?}",
                self.alerts,
                computer
                    .runs
                    .iter()
                    .map(|view| (&view.id, &view.status, users(view)))
                    .collect::<Vec<_>>()
            );
        }
        for run in &self.closed {
            assert!(computer.closed.contains(run), "{run:?} is not closed");
        }
        for (run, goal) in &self.goals {
            let state: tau_goal::ui::State = computer
                .runs
                .iter()
                .find(|view| &view.id == run)
                .and_then(|view| view.plugin_states.get(tau_goal::NAME))
                .map(|state| {
                    serde_json::from_value(state.json().clone()).unwrap()
                })
                .unwrap_or_default();
            let shown = state.goal.map(|shown| shown.condition);
            assert_eq!(shown.as_ref(), Some(goal), "{run:?}'s goal");
        }
        for run in &self.ending {
            let ended = computer
                .runs
                .iter()
                .any(|view| &view.id == run && view.ending.is_some());
            let waiting = computer.runs.iter().any(|view| {
                view.landing_queue
                    .iter()
                    .any(|waiting| *waiting.run == *run.0)
            });
            assert!(
                ended || waiting,
                "{run:?} neither ended nor waits to land"
            );
        }
        if let Some(run) = &self.kept {
            assert_eq!(
                computer.kept_branch.as_ref(),
                Some(run),
                "the kept branch"
            );
        }
        for repo in &self.hidden_repos {
            assert!(
                computer
                    .catalog
                    .repos
                    .iter()
                    .all(|listed| &listed.name != repo),
                "{repo} is still listed"
            );
        }
        let settings = &computer.catalog.models.settings;
        for (id, hidden) in &self.hidden {
            assert_eq!(settings.is_hidden(id), *hidden, "{id} hidden");
        }
        if let Some(choice) = &self.default {
            assert_eq!(&settings.default_for("coder"), choice, "coder's model");
        }
        for run in &self.cancelled {
            let view = computer.runs.iter().find(|view| &view.id == run);
            assert!(
                view.is_some_and(|view| !view.status.is_live()),
                "{run:?} still runs after it was cancelled"
            );
        }
    }
}

struct Tau {
    /// The computer's host, to ask whether it is still at work.
    host: Arc<Host>,
    /// Keeps the app the windows are in.
    _app: TestAppContext,
    computer: Device,
    phones: Vec<Device>,
    wire: Rc<RefCell<Wire>>,
    gate: Gate,
    /// What the host got, from any device.
    asked: Rc<RefCell<Asked>>,
    /// The last prompt's number: each is told apart.
    prompts: usize,
}

/// Who acts: the computer, or a phone.
#[derive(Debug, Clone, Copy)]
enum Who {
    Computer,
    Phone(usize),
}

impl Tau {
    fn new(phones: usize) -> Self {
        let app = TestAppContext::single();
        app.executor().allow_parking();
        app.update(tau_ui_remote::init);
        let gate = Gate::default();
        let (host, events) = host(&gate);
        let catalog = host.block_on(host.catalog());
        let mut cx = app.clone();
        let window = cx.add_window(|window, cx| {
            Workspace::new("tau", Vec::new(), catalog, window, cx)
        });
        let workspace = window.root(&mut cx).unwrap();
        let mut computer_cx =
            VisualTestContext::from_window(window.into(), &cx);
        let host =
            computer_cx.update(|_, cx| host.attach(&workspace, events, cx));
        // What the host gets: the computer's own requests, and phones'
        // as they arrive.
        let asked = Rc::new(RefCell::new(Asked::default()));
        let arrived = asked.clone();
        computer_cx.update(|_, cx| {
            cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
                arrived.borrow_mut().arrived(event)
            })
            .detach();
        });

        let wire = Rc::new(RefCell::new(Wire {
            feed: Feed::new(1_000),
            links: (0..phones)
                .map(|_| Link {
                    conn: None,
                    down: VecDeque::new(),
                    up: VecDeque::new(),
                    outbox: Outbox::new(0),
                    taken: None,
                    last: None,
                    waiting: false,
                })
                .collect(),
            next_conn: 0,
        }));
        // What the host applies goes out, as `phone_server` sends it.
        let out = wire.clone();
        computer_cx.update(|_, cx| {
            workspace.update(cx, |ws, _| ws.set_mirrored(true));
            cx.subscribe(&workspace, move |_, update, _| {
                if let Some(body) = phone_server::to_phones(update) {
                    out.borrow_mut().broadcast(body);
                }
            })
            .detach();
            let answered = asked.clone();
            cx.subscribe(&workspace, move |_, update: &HostUpdate, _| {
                answered.borrow_mut().answered(update)
            })
            .detach();
        });
        let computer = Device {
            workspace,
            cx: computer_cx,
        };

        let phones = (0..phones)
            .map(|phone| {
                let mut cx = app.clone();
                let window = cx.add_window(|window, cx| {
                    Workspace::new(
                        "phone",
                        Vec::new(),
                        Default::default(),
                        window,
                        cx,
                    )
                });
                let workspace = window.root(&mut cx).unwrap();
                let mut phone_cx =
                    VisualTestContext::from_window(window.into(), &cx);
                // What the person asks for goes up, as `remote` sends it.
                let up = wire.clone();
                phone_cx.update(|_, cx| {
                    cx.subscribe(
                        &workspace,
                        move |_, event: &WorkspaceEvent, _| {
                            if !event.from_phone() {
                                return;
                            }
                            let up_event = PhoneUp::Event(event.clone());
                            let Some(body) = remote::to_computer(&up_event)
                            else {
                                return;
                            };
                            // Sent now if connected, and kept until taken.
                            let mut wire = up.borrow_mut();
                            let link = &mut wire.links[phone];
                            let request = link.outbox.push(body);
                            if link.conn.is_some() {
                                link.up.push_back(request);
                            }
                        },
                    )
                    .detach();
                });
                wire.borrow_mut().connect(phone);
                Device {
                    workspace,
                    cx: phone_cx,
                }
            })
            .collect();
        let mut tau = Self {
            _app: app,
            computer,
            phones,
            wire,
            host,
            gate,
            asked,
            prompts: 0,
        };
        tau.settle();
        // The first repository's main chat has a turn, which new chats
        // can start from.
        let main = tau.computer.runs()[0].id.clone();
        tau.say_to(Who::Computer, &main, "the first turn".into());
        tau.host_settles();
        tau.gate.answer(0, Reply::Text);
        tau.settle();
        tau
    }

    fn device(&mut self, who: Who) -> &mut Device {
        match who {
            Who::Computer => &mut self.computer,
            Who::Phone(phone) => &mut self.phones[phone],
        }
    }

    /// Someone who can act: the computer, or a connected phone.
    fn who(&self, tc: &TestCase) -> Who {
        // Phones mostly: what crosses the network is what can go wrong.
        if !tc.draw(gs::weighted_booleans(0.75)) {
            return Who::Computer;
        }
        // Connected or not: what it asks goes once it reaches the
        // computer.
        let last = self.phones.len() - 1;
        Who::Phone(tc.draw(gs::integers().min_value(0).max_value(last)))
    }

    fn prompt(&mut self) -> String {
        self.prompts += 1;
        format!("prompt {}", self.prompts)
    }

    /// Runs the computer until its host has nothing left to do: every
    /// run it shows going on waits for the model, and nothing changes a
    /// while.
    fn host_settles(&mut self) {
        let deadline = Instant::now() + SETTLE;
        let mut last = None;
        let mut still = 0;
        while still < 2 {
            assert!(Instant::now() < deadline, "the host never settled");
            self.computer.cx.run_until_parked();
            let synced = self.computer.synced();
            // Every live run, sub-agents included, waits for the model:
            // nothing waits for a sub-agent (ADR 0026).
            let live = synced
                .runs
                .iter()
                .filter(|run| run.status.is_live())
                .count();
            let sessions = self.gate.sessions.load(Ordering::SeqCst);
            let idle = sessions == live
                && self.gate.waiting() == live
                && !self.host.busy();
            let now =
                (synced, self.wire.borrow().feed.seq(), self.gate.waiting());
            if idle && last.as_ref() == Some(&now) {
                still += 1;
            } else {
                still = 0;
                last = Some(now);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Delivers the next message for `phone`: from the computer when
    /// `down`, else to it. Returns whether there was one.
    fn deliver(&mut self, phone: usize, down: bool) -> bool {
        if down {
            let next = self.wire.borrow_mut().links[phone].down.pop_front();
            let Some(next) = next else { return false };
            if let Down::Ack { up } = next {
                self.wire.borrow_mut().links[phone].outbox.taken(up);
                return true;
            }
            let seq = next.seq();
            let device = &mut self.phones[phone];
            for update in remote::updates(next) {
                device
                    .workspace
                    .update(&mut device.cx, |ws, cx| ws.apply(update, cx));
            }
            device.cx.run_until_parked();
            self.wire.borrow_mut().links[phone].last = seq;
        } else {
            let next = self.wire.borrow_mut().links[phone].up.pop_front();
            let Some(Up::Up { id, body }) = next else {
                return false;
            };
            // Taken once, and answered each time, as the server does.
            let taken = {
                let mut wire = self.wire.borrow_mut();
                let link = &mut wire.links[phone];
                link.down.push_back(Down::Ack { up: id });
                let new = outbox::is_new(link.taken, id);
                if new {
                    link.taken = Some(id);
                }
                new
            };
            if !taken {
                return true;
            }
            match phone_server::from_phone(body) {
                Some(PhoneUp::Event(event)) => {
                    let computer = &mut self.computer;
                    computer
                        .workspace
                        .update(&mut computer.cx, |_, cx| cx.emit(event));
                }
                Some(PhoneUp::Name(_)) | None => {}
            }
            self.host_settles();
        }
        true
    }

    /// The computer sends the snapshot `phone` waits for.
    fn send_snapshot(&mut self, phone: usize) {
        let bodies = self
            .computer
            .workspace
            .read_with(&self.computer.cx, |ws, _| phone_server::snapshot(ws));
        let mut wire = self.wire.borrow_mut();
        let conn = wire.links[phone].conn.unwrap();
        let down = wire.feed.snapshot(conn, bodies).unwrap();
        let link = &mut wire.links[phone];
        link.waiting = false;
        link.down.push_back(down);
    }

    /// Everything arrives, phones that dropped come back, and the host
    /// settles.
    fn settle(&mut self) {
        self.host_settles();
        loop {
            let mut moved = false;
            for phone in 0..self.phones.len() {
                let (conn, waiting) = {
                    let wire = self.wire.borrow();
                    let link = &wire.links[phone];
                    (link.conn, link.waiting)
                };
                if conn.is_none() {
                    self.wire.borrow_mut().connect(phone);
                    moved = true;
                    continue;
                }
                if waiting {
                    self.send_snapshot(phone);
                    moved = true;
                }
                while self.deliver(phone, false) {
                    moved = true;
                }
                while self.deliver(phone, true) {
                    moved = true;
                }
            }
            if !moved {
                break;
            }
            self.host_settles();
        }
        assert!(self.wire.borrow().settled());
    }

    /// The person's message to `run`, typed on `who`.
    fn say_to(&mut self, who: Who, run: &RunId, text: String) {
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.navigate(Route::Run(run.clone()), cx);
            ws.submit_prompt(text, cx);
        });
        device.cx.run_until_parked();
    }

    fn after_action(&mut self, who: Who) {
        if let Who::Computer = who {
            self.host_settles();
        }
    }
}

/// One of `runs`, which are not empty.
fn pick(tc: &TestCase, runs: &[RunView]) -> RunId {
    let n = tc.draw(gs::integers().min_value(0).max_value(runs.len() - 1));
    runs[n].id.clone()
}

/// A host on `gate`, listing two repositories.
fn host(
    gate: &Gate,
) -> (
    Host,
    tokio::sync::mpsc::UnboundedReceiver<tau_agent::event::RunEvent>,
) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let dir = tempfile::tempdir().unwrap().keep();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-6-luna".into()),
        store: dir.join("unused.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
    };
    let agent = Agent::new(gate.clone()).name("coder");
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    // Memory's pass after each run asks the model once more: these
    // scripts answer only the requests they write.
    let host = host.with_plugin_settings(
        tau_memory::NAME,
        serde_json::json!({ "after_each_run": false }),
    );
    // Forecasts start at once: no person's burst of changes to wait out.
    let host = host.with_forecast_wait(Duration::from_millis(1));
    let host = REPOS.iter().fold(host, |host, repo| {
        host.with_repo(repo, project(&dir.join(repo).join("checkout")))
    });
    (host, events)
}

fn project(checkout: &Path) -> Project {
    std::fs::create_dir_all(checkout).unwrap();
    git(checkout, &["init", "--quiet"]);
    git(
        checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        checkout.with_file_name("project"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap()
}

#[hegel::state_machine]
impl Tau {
    /// The person starts a chat.
    #[rule]
    fn new_chat(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let text = self.prompt();
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.navigate(Route::NewRun, cx);
            ws.submit_prompt(text.clone(), cx);
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person writes to a chat: a live one is steered, a finished
    /// one goes on.
    #[rule(weight = 3)]
    fn message(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let runs = self.device(who).runs();
        tc.assume(!runs.is_empty());
        let run = pick(&tc, &runs);
        let text = self.prompt();
        self.say_to(who, &run, text);
        self.after_action(who);
    }

    /// The person closes a chat.
    #[rule]
    fn close(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let runs = self.device(who).runs();
        tc.assume(!runs.is_empty());
        let run = pick(&tc, &runs);
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.close_run(&run, cx);
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person stops a live chat.
    #[rule]
    fn cancel(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let live: Vec<RunView> = self
            .device(who)
            .runs()
            .into_iter()
            .filter(|run| run.status.is_live())
            .collect();
        tc.assume(!live.is_empty());
        let run = pick(&tc, &live);
        let device = self.device(who);
        let event = WorkspaceEvent::Cancel { run: run.clone() };
        device
            .workspace
            .update(&mut device.cx, |_, cx| cx.emit(event));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person sets a chat's goal: tau-goal's UI stores a record.
    #[rule]
    fn set_goal(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let runs = self.device(who).runs();
        tc.assume(!runs.is_empty());
        let run = pick(&tc, &runs);
        let goal = self.prompt();
        let device = self.device(who);
        let handle = device
            .workspace
            .read_with(&device.cx, |ws, _| ws.plugin_handle(tau_goal::NAME));
        let record = tau_goal::Record::Set {
            goal,
            continuations: 1,
            budget: 1.0,
        };
        device.cx.update(|_, cx| handle.record(&run, record, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person shows or hides a model in the picker.
    #[rule]
    fn hide_model(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let id = tc.draw(gs::sampled_from(models()));
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.toggle_model_hidden(&id, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person picks the model new chats start on.
    #[rule]
    fn pick_default_model(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let model = tc.draw(gs::sampled_from(models()));
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            let choice = ModelChoice::new(model.clone(), Effort::Auto);
            ws.set_default_model("coder", choice, cx)
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person starts a chat from a main chat's finished turn.
    #[rule(weight = 2)]
    fn fork(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let device = self.device(who);
        let mains: Vec<(RunId, u32)> =
            device.workspace.read_with(&device.cx, |ws, _| {
                ws.runs()
                    .iter()
                    .filter(|run| ws.is_main(&run.id))
                    .map(|run| {
                        (run.id.clone(), Workspace::last_fork_turn_of(run))
                    })
                    .filter(|(_, turn)| *turn >= 1)
                    .collect()
            });
        tc.assume(!mains.is_empty());
        let n = tc.draw(gs::integers().min_value(0).max_value(mains.len() - 1));
        let (main, last) = mains[n].clone();
        let turn = tc.draw(gs::integers().min_value(1).max_value(last));
        let text = self.prompt();
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            assert!(ws.start_fork_at(&main, turn, cx));
            ws.submit_prompt(text, cx);
        });
        tc.event("a chat started from a turn");
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person lands a finished chat on its main chat, drops it, or
    /// keeps its branch.
    #[rule(weight = 3)]
    fn end_chat(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let device = self.device(who);
        let chats: Vec<RunView> = device
            .runs()
            .into_iter()
            .filter(|run| {
                matches!(run.origin, Origin::Fork { .. })
                    && !run.status.is_live()
                    && run.ending.is_none()
            })
            .collect();
        tc.assume(!chats.is_empty());
        let run = pick(&tc, &chats);
        let how = tc.draw(gs::sampled_from(vec!["land", "drop", "keep"]));
        tc.event(format!("a chat asked to {how}"));
        device.workspace.update(&mut device.cx, |ws, cx| match how {
            "land" => ws.land(&run, cx),
            "drop" => ws.drop_child(&run, cx),
            _ => ws.keep_branch(&run, cx),
        });
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The person stops listing a repository, while another is listed.
    #[rule]
    fn hide_repo(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let device = self.device(who);
        let repos: Vec<String> =
            device.workspace.read_with(&device.cx, |ws, _| {
                ws.catalog()
                    .repos
                    .iter()
                    .map(|repo| repo.name.clone())
                    .collect()
            });
        tc.assume(repos.len() > 1);
        let repo = tc.draw(gs::sampled_from(repos));
        tc.event("a repository hidden");
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.remove_repo(&repo, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// Someone writes to a finished main chat, and the model hands a
    /// task to a sub-agent: a chat of its own, under main, on every
    /// device.
    #[rule(weight = 2)]
    fn main_spawns(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let mains: Vec<RunView> =
            self.computer
                .workspace
                .read_with(&self.computer.cx, |ws, _| {
                    ws.runs()
                        .iter()
                        .filter(|run| {
                            ws.is_main(&run.id) && !run.status.is_live()
                        })
                        .cloned()
                        .collect()
                });
        tc.assume(!mains.is_empty());
        let main = pick(&tc, &mains);
        let waiting = self.gate.waiting();
        let text = self.prompt();
        self.say_to(who, &main, text);
        // A phone's message goes up first.
        if let Who::Phone(phone) = who {
            while self.deliver(phone, false) {}
        }
        self.host_settles();
        // Main went on, unless the host had it running by now: then the
        // message steered it, and there is nothing to hand off. (Too late
        // to reject: the message went.)
        if self.gate.waiting() != waiting + 1 {
            return;
        }
        tc.event("a main chat hands off a task");
        self.gate.answer(
            waiting,
            Reply::Delegate {
                task: "write the notes".into(),
            },
        );
        self.host_settles();
    }

    /// A chat starts from main, writes a file and finishes, and the
    /// person lands it from a device that shows it finished.
    #[rule(weight = 2)]
    fn chat_lands(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let main =
            self.computer
                .workspace
                .read_with(&self.computer.cx, |ws, _| {
                    ws.runs()
                        .iter()
                        .find(|run| {
                            ws.is_main(&run.id)
                                && Workspace::last_fork_turn_of(run) >= 1
                        })
                        .map(|run| run.id.clone())
                });
        let Some(main) = main else {
            tc.event("no main chat to land on");
            tc.reject()
        };
        let waiting = self.gate.waiting();
        let text = self.prompt();
        let computer = &mut self.computer;
        computer.workspace.update(&mut computer.cx, |ws, cx| {
            if ws.start_fork_at(&main, 1, cx) {
                ws.submit_prompt(text.clone(), cx);
            }
        });
        self.host_settles();
        // Its turn writes a file, then it is done, and done again when
        // tau-vcs holds its stop once to have it commit; something else
        // may have taken the turn first.
        for reply in [
            Reply::Write {
                path: "c.txt".into(),
                content: format!("{text}\n"),
            },
            Reply::Text,
            Reply::Text,
        ] {
            if self.gate.waiting() != waiting + 1 {
                tc.event("a chat to land was taken over");
                return;
            }
            self.gate.answer(waiting, reply);
            self.host_settles();
        }
        let chat = self.computer.runs().into_iter().find(|run| {
            matches!(run.origin, Origin::Fork { .. })
                && run.items.contains(&Item::User(text.clone()))
                && !run.status.is_live()
        });
        let Some(chat) = chat else {
            tc.event("a chat to land did not finish");
            return;
        };
        // The device sees it finished before it lands it.
        self.settle();
        tc.event("a finished chat lands");
        let device = self.device(who);
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.land(&chat.id, cx));
        device.cx.run_until_parked();
        self.after_action(who);
    }

    /// The model answers one waiting request.
    #[rule(weight = 3)]
    fn model_answers(&mut self, tc: TestCase) {
        let waiting = self.gate.waiting();
        tc.assume(waiting > 0);
        let n = tc.draw(gs::integers().min_value(0).max_value(waiting - 1));
        let text = self.prompt();
        // Mostly done; else a file, which two chats may both write, or a
        // sub-agent's task.
        let reply =
            match tc.draw(gs::integers::<u8>().min_value(0).max_value(3)) {
                0 | 1 => Reply::Text,
                2 => Reply::Write {
                    path: tc
                        .draw(gs::sampled_from(vec!["a.txt", "b.txt"]))
                        .to_owned(),
                    content: format!("{text}\n"),
                },
                _ => Reply::Delegate {
                    task: "write the notes".into(),
                },
            };
        tc.event(match &reply {
            Reply::Text => "the model is done",
            Reply::Write { .. } => "the model writes a file",
            Reply::Delegate { .. } => "the model hands off a task",
        });
        self.gate.answer(n, reply);
        self.host_settles();
    }

    /// One message crosses the network, one way or the other.
    #[rule(weight = 4)]
    fn one_arrives(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        let down: bool = tc.draw(gs::booleans());
        if !self.deliver(phone, down) {
            tc.reject();
        }
    }

    /// A phone's connection drops: what was on its way is lost.
    #[rule]
    fn phone_drops(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        let mut wire = self.wire.borrow_mut();
        let link = &mut wire.links[phone];
        // The network loses what was on its way, both ways, answers
        // included; the phone's outbox keeps what was not taken.
        let Some(conn) = link.conn.take() else {
            tc.reject()
        };
        link.down.clear();
        link.up.clear();
        link.waiting = false;
        wire.feed.leave(conn);
    }

    /// The computer takes a phone's request, and the connection drops
    /// before its answer arrives: the phone sends it again on the next.
    #[rule(weight = 3)]
    fn answer_lost(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        if !self.deliver(phone, false) {
            tc.reject();
        }
        tc.event("an answer was lost");
        let mut wire = self.wire.borrow_mut();
        let link = &mut wire.links[phone];
        let conn = link.conn.take().expect("it was connected");
        link.down.clear();
        link.up.clear();
        link.waiting = false;
        wire.feed.leave(conn);
    }

    /// A phone that dropped connects again, from the last message it
    /// had.
    #[rule]
    fn phone_comes_back(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        tc.assume(self.wire.borrow().links[phone].conn.is_none());
        self.wire.borrow_mut().connect(phone);
    }

    /// The computer answers a phone that needs everything.
    #[rule]
    fn snapshot_sent(&mut self, tc: TestCase) {
        let phone = tc
            .draw(gs::integers().min_value(0).max_value(self.phones.len() - 1));
        tc.assume(self.wire.borrow().links[phone].waiting);
        self.send_snapshot(phone);
    }

    /// Everything arrives: every phone shows what the computer shows,
    /// and the computer shows what was asked of any of them.
    #[rule(weight = 2)]
    fn all_arrive(&mut self, tc: TestCase) {
        self.settle();
        let computer = self.computer.synced();
        if computer
            .runs
            .iter()
            .any(|run| matches!(run.origin, Origin::SubAgent { .. }))
        {
            tc.event("a sub-agent's chat is shown");
        }
        for (phone, device) in self.phones.iter_mut().enumerate() {
            let theirs = device.synced();
            assert_synced(phone, &computer, &theirs);
        }
        self.asked.borrow().shown_by(&computer);
    }
}

/// Fails with the first field the phone shows otherwise.
fn assert_synced(phone: usize, computer: &Synced, theirs: &Synced) {
    if computer == theirs {
        return;
    }
    macro_rules! field {
        ($($name:ident),*) => {$(
            assert_eq!(
                theirs.$name, computer.$name,
                "phone {phone} shows another `{}` than the computer",
                stringify!($name)
            );
        )*};
    }
    for (ours, phone_run) in computer.runs.iter().zip(&theirs.runs) {
        assert_eq!(phone_run, ours, "phone {phone} shows the run otherwise");
    }
    field!(
        runs,
        catalog,
        queued,
        resuming,
        closed,
        kept_branch,
        pushes,
        proposed,
        pull_requests,
        branch_code,
        query_result
    );
    unreachable!("Synced compared unequal with every field equal");
}

// A real host per case, so generating is slow by nature.
#[hegel::test(test_cases = 30, suppress_health_check = [HealthCheck::TooSlow])]
fn every_device_shows_what_any_device_did(tc: TestCase) {
    let phones = tc.draw(gs::integers().min_value(1).max_value(2));
    let tau = Tau::new(phones);
    hegel::stateful::machine(tau).steps(30).run(tc);
}

/// A phone's message the computer took, its answer lost with the
/// connection, is sent again on the next and shows once.
#[test]
fn a_message_whose_answer_was_lost_shows_once() {
    let mut tau = Tau::new(1);
    let main = tau.computer.runs()[0].id.clone();
    tau.say_to(Who::Phone(0), &main, "hello".into());
    assert!(tau.deliver(0, false), "it went up");
    {
        let mut wire = tau.wire.borrow_mut();
        let link = &mut wire.links[0];
        let conn = link.conn.take().unwrap();
        link.down.clear();
        wire.feed.leave(conn);
    }
    tau.settle();
    let view = tau.computer.runs().into_iter().find(|run| run.id == main);
    let said = view
        .unwrap()
        .items
        .iter()
        .filter(|item| matches!(item, Item::User(text) if text == "hello"))
        .count();
    assert_eq!(said, 1);
    assert!(tau.asked.borrow().twice.is_empty());
}
