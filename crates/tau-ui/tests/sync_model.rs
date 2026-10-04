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
    feed::{Feed, Joined},
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
    view::{Item, RunView},
    workspace::Synced,
};
use tau_vcs::{Identity, Project};
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

/// The repository the host lists.
const REPO: &str = "repo";
/// How long the host has to settle before the test fails.
const SETTLE: Duration = Duration::from_secs(30);

/// A model that answers each request only when [`Gate::answer`] says.
#[derive(Clone, Default)]
struct Gate {
    model: ScriptedModel,
    waiting: Arc<Mutex<Vec<oneshot::Sender<()>>>>,
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

    /// Whether every run going on waits for the model: nothing else
    /// happens until the test answers.
    fn idle(&self) -> bool {
        self.sessions.load(Ordering::SeqCst) == self.waiting()
    }

    /// Answers the `n`th waiting request: the turn ends with a reply.
    fn answer(&self, n: usize) {
        let answer = self.waiting.lock().unwrap().remove(n);
        let _ = answer.send(());
    }
}

impl Llm for Gate {
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        let (gate, open) = (self.clone(), self.model.open(settings));
        gate.sessions.fetch_add(1, Ordering::SeqCst);
        async move {
            let inner = open.await?;
            Ok(Box::new(GatedSession { gate, inner }) as Box<dyn LlmSession>)
        }
        .boxed()
    }
}

struct GatedSession {
    gate: Gate,
    inner: Box<dyn LlmSession>,
}

impl Drop for GatedSession {
    fn drop(&mut self) {
        self.gate.sessions.fetch_sub(1, Ordering::SeqCst);
    }
}

impl LlmSession for GatedSession {
    fn settings(&self) -> &Settings {
        self.inner.settings()
    }

    fn set_reasoning(
        &mut self,
        effort: Option<tau_ai::responses::request::ReasoningEffort>,
    ) {
        self.inner.set_reasoning(effort);
    }

    fn respond(
        &mut self,
        transcript: &[Message],
        timestamp: Timestamp,
    ) -> EventStream {
        let _ = self.gate.model.clone().turn(|turn| turn.text("Done."));
        let reply = self.inner.respond(transcript, timestamp);
        let (answer, answered) = oneshot::channel();
        self.gate.waiting.lock().unwrap().push(answer);
        stream::once(answered.map(move |_| reply)).flatten().boxed()
    }
}

/// A phone's end of the network.
#[derive(Default)]
struct Link {
    /// Its connection, while it has one.
    conn: Option<u32>,
    /// What the computer sent that has not arrived.
    down: VecDeque<Down>,
    /// What the phone sent that has not arrived.
    up: VecDeque<Value>,
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
    }

    fn settled(&self) -> bool {
        self.links.iter().all(|link| {
            link.conn.is_some()
                && link.down.is_empty()
                && link.up.is_empty()
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
    /// Each message, its run (none for a new chat), and whether the run
    /// was stopped after it: a steer it had not read goes with it.
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
}

impl Asked {
    /// The host got `event`.
    fn arrived(&mut self, event: &WorkspaceEvent) {
        match event {
            WorkspaceEvent::NewRun { prompt, .. } => {
                self.said.push((None, prompt.clone(), false))
            }
            WorkspaceEvent::Say { run, text, .. } => {
                self.said.push((Some(run.clone()), text.clone(), false));
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

    /// Fails unless `computer` shows what was asked.
    fn shown_by(&self, computer: &Synced) {
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
            in_transcript || queued.is_some_and(|texts| texts.contains(text))
        };
        for (run, text, stopped) in &self.said {
            assert!(
                *stopped || shows(run.as_ref(), text),
                "the computer does not show {text:?} to {run:?}; it shows {:?}",
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
        let catalog = host.catalog();
        let mut cx = app.clone();
        let window = cx.add_window(|window, cx| {
            Workspace::new("tau", Vec::new(), catalog, window, cx)
        });
        let workspace = window.root(&mut cx).unwrap();
        let mut computer_cx =
            VisualTestContext::from_window(window.into(), &cx);
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
            links: (0..phones).map(|_| Link::default()).collect(),
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
                            if let Some(body) = remote::to_computer(&up_event) {
                                up.borrow_mut().links[phone].up.push_back(body);
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
            gate,
            asked,
            prompts: 0,
        };
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
        let phones = self.phones.len();
        let pick: usize =
            tc.draw(gs::integers().min_value(0).max_value(phones));
        if pick == phones {
            return Who::Computer;
        }
        let wire = self.wire.borrow();
        let link = &wire.links[pick];
        // A phone that is not connected says so (decision 0013); what it
        // sends then is another property's.
        tc.assume(link.conn.is_some() && !link.waiting);
        Who::Phone(pick)
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
        let started = Instant::now();
        let mut polls = 0;
        let mut last = None;
        let mut still = 0;
        while still < 3 {
            assert!(Instant::now() < deadline, "the host never settled");
            self.computer.cx.run_until_parked();
            let synced = self.computer.synced();
            let live = synced
                .runs
                .iter()
                .filter(|run| run.status.is_live())
                .count();
            let idle = self.gate.idle() && live == self.gate.waiting();
            let now =
                (synced, self.wire.borrow().feed.seq(), self.gate.waiting());
            if idle && last.as_ref() == Some(&now) {
                still += 1;
            } else {
                still = 0;
                last = Some(now);
            }
            std::thread::sleep(Duration::from_millis(5));
            polls += 1;
            if polls % 100 == 0 {
                eprintln!(
                    "PROFILE slow: sessions {} waiting {} live {}",
                    self.gate.sessions.load(Ordering::SeqCst),
                    self.gate.waiting(),
                    live
                );
            }
        }
        if started.elapsed() > Duration::from_millis(300) {
            eprintln!("PROFILE settle {:?}", started.elapsed());
        }
    }

    /// Delivers the next message for `phone`: from the computer when
    /// `down`, else to it. Returns whether there was one.
    fn deliver(&mut self, phone: usize, down: bool) -> bool {
        if down {
            let next = self.wire.borrow_mut().links[phone].down.pop_front();
            let Some(next) = next else { return false };
            let seq = next.seq();
            let device = &mut self.phones[phone];
            for update in remote::updates(next) {
                device
                    .workspace
                    .update(&mut device.cx, |ws, cx| ws.apply(update, cx));
            }
            device.cx.run_until_parked();
            self.wire.borrow_mut().links[phone].last = Some(seq);
        } else {
            let next = self.wire.borrow_mut().links[phone].up.pop_front();
            let Some(next) = next else { return false };
            match phone_server::from_phone(next) {
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

/// A host on `gate`, listing one repository.
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
    let store = runtime.block_on(tau_store::Store::memory()).unwrap();
    let dir = tempfile::tempdir().unwrap().keep();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(dir.join("config")),
        model: Some("gpt-6-luna".into()),
        store: dir.join("unused.db"),
        repos: dir.join("repos"),
        settings: dir.join("models.json"),
        repo_list: dir.join("repos.json"),
    };
    let agent = Agent::new(gate.clone()).name("coder");
    let (host, events) = Host::with_agent(runtime, agent, store, config);
    (host.with_repo(REPO, project(&dir.join("checkout"))), events)
}

fn project(checkout: &Path) -> Project {
    std::fs::create_dir_all(checkout).unwrap();
    git(checkout, &["init", "--quiet"]);
    git(
        checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    Project::import(
        checkout.to_str().unwrap(),
        checkout.with_file_name("project"),
        Identity::default(),
    )
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

    /// The model answers one waiting request.
    #[rule(weight = 3)]
    fn model_answers(&mut self, tc: TestCase) {
        let waiting = self.gate.waiting();
        tc.assume(waiting > 0);
        let n = tc.draw(gs::integers().min_value(0).max_value(waiting - 1));
        self.gate.answer(n);
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
        // What the phone sent waits for the next connection; the
        // network loses what the computer had sent.
        let Some(conn) = link.conn.take() else {
            tc.reject()
        };
        link.down.clear();
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
    fn all_arrive(&mut self, _tc: TestCase) {
        self.settle();
        let computer = self.computer.synced();
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
    let t = Instant::now();
    let phones = tc.draw(gs::integers().min_value(1).max_value(2));
    let tau = Tau::new(phones);
    hegel::stateful::machine(tau).steps(25).run(tc);
    eprintln!("PROFILE case {:?}", t.elapsed());
}
