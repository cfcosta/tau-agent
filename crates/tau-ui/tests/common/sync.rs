//! One tau, its computer's window and phones (decision 0013), in
//! process: what the sync models share.
//!
//! The computer runs the real host, with a model that answers only when
//! the test says so, so runs stay live as long as the test wants. The
//! phones reach it through the real feed and the real encoding: what
//! the network would do is the test's to pick, message by message,
//! including dropping a phone and bringing it back, and closing tau and
//! starting it again on what it left on disk.

#![allow(dead_code, reason = "each model that includes this uses a part of it")]

use std::{
    cell::RefCell,
    collections::VecDeque,
    path::{Path, PathBuf},
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
use hegel::{TestCase, generators as gs};
use serde_json::Value;
use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
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
    phones::PhoneUp,
    remote,
    route::Route,
    view::RunView,
    workspace::Synced,
};
use tau_vcs_host::{Identity, Project, ProjectRepo};
use tokio::sync::{mpsc::UnboundedReceiver, oneshot};

/// The models people show, hide and pick: the plan's.
pub fn models() -> Vec<String> {
    let offered = tau_ai::model::plan_models();
    offered
        .iter()
        .take(3)
        .map(|model| model.id.clone())
        .collect()
}

/// The repositories the host lists.
pub const REPOS: [&str; 2] = ["repo", "other"];
/// How long the host has to settle before the test fails.
pub const SETTLE: Duration = Duration::from_secs(30);

/// What the model answers a request with.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
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
pub struct Gate {
    model: ScriptedModel,
    waiting: Arc<Mutex<Vec<oneshot::Sender<Reply>>>>,
    /// Sessions open: one per run going on.
    pub sessions: Arc<AtomicUsize>,
}

impl Gate {
    /// The requests still waiting: a cancelled run's went away.
    pub fn waiting(&self) -> usize {
        let mut waiting = self.waiting.lock().unwrap();
        waiting.retain(|answer| !answer.is_closed());
        waiting.len()
    }

    /// Answers the `n`th waiting request with `reply`.
    pub fn answer(&self, n: usize, reply: Reply) {
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
pub struct Link {
    /// Its connection, while it has one.
    pub conn: Option<u32>,
    /// What the computer sent that has not arrived, answers included.
    pub down: VecDeque<Down>,
    /// What the phone sent on its connection that has not arrived.
    pub up: VecDeque<Up>,
    /// What the person asked on the phone that the computer has not
    /// taken, as `remote` keeps it.
    pub outbox: Outbox,
    /// The last request the computer took from it, as `phone_server`
    /// keeps it: saved with the paired devices, so it outlives a
    /// restart of the computer.
    pub taken: Option<u64>,
    /// The last request the computer took from it, as it came: what a
    /// network that repeats would bring again.
    pub last_taken: Option<(u64, Value)>,
    /// The last message's number it applied.
    pub last: Option<u64>,
    /// It needs a snapshot the computer has not sent.
    pub waiting: bool,
}

impl Link {
    fn new() -> Self {
        Self {
            conn: None,
            down: VecDeque::new(),
            up: VecDeque::new(),
            outbox: Outbox::new(0),
            taken: None,
            last_taken: None,
            last: None,
            waiting: false,
        }
    }
}

/// The network between the computer and its phones.
pub struct Wire {
    pub feed: Feed<u32>,
    pub links: Vec<Link>,
    next_conn: u32,
}

impl Wire {
    pub fn broadcast(&mut self, body: Value) {
        let (down, to) = self.feed.broadcast(body);
        for link in &mut self.links {
            if link.conn.is_some_and(|conn| to.contains(&conn)) {
                link.down.push_back(down.clone());
            }
        }
    }

    pub fn connect(&mut self, phone: usize) {
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

    /// `phone`'s connection drops: what was on its way is lost, both
    /// ways, answers included; its outbox keeps what was not taken.
    /// Returns whether it was connected.
    pub fn drop_phone(&mut self, phone: usize) -> bool {
        let link = &mut self.links[phone];
        let Some(conn) = link.conn.take() else {
            return false;
        };
        link.down.clear();
        link.up.clear();
        link.waiting = false;
        self.feed.leave(conn);
        true
    }

    pub fn settled(&self) -> bool {
        self.links.iter().all(|link| {
            link.conn.is_some()
                && link.down.is_empty()
                && link.up.is_empty()
                && link.outbox.is_empty()
                && !link.waiting
        })
    }
}

pub struct Device {
    pub workspace: Entity<Workspace>,
    pub cx: VisualTestContext,
}

impl Device {
    pub fn synced(&mut self) -> Synced {
        self.workspace.read_with(&self.cx, |ws, _| ws.synced())
    }

    pub fn runs(&mut self) -> Vec<RunView> {
        self.workspace
            .read_with(&self.cx, |ws, _| ws.runs().to_vec())
    }
}

/// Who acts: the computer, or a phone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Who {
    Computer,
    Phone(usize),
}

/// Where the host keeps what it keeps: its store in memory, or on disk
/// beside everything else, so tau can close and start again on it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Keep {
    Memory,
    Disk,
}

/// What outlives the computer's host: its directory (the store, the
/// repository list, the UI state, the models' settings) and the
/// repositories' projects.
struct Disk {
    dir: PathBuf,
    keep: Keep,
}

impl Disk {
    fn new(keep: Keep) -> Self {
        let dir = tempfile::tempdir().unwrap().keep();
        for repo in REPOS {
            new_project(&dir.join(repo).join("checkout"));
        }
        Self { dir, keep }
    }

    /// A host started on what is on disk, as tau starts.
    fn start(&self, gate: &Gate) -> (Host, UnboundedReceiver<RunEvent>) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let dir = &self.dir;
        let store = match self.keep {
            Keep::Memory => runtime.block_on(tau_store_sqlite::memory()),
            Keep::Disk => {
                runtime.block_on(tau_store_sqlite::open(dir.join("runs.db")))
            }
        }
        .unwrap();
        // tau keeps the main chats of repositories from GitHub across
        // starts, as these stand for.
        let list = dir.join("repos.json");
        if let Ok(text) = std::fs::read_to_string(&list) {
            let mut saved: Value = serde_json::from_str(&text).unwrap();
            for repo in saved["repos"].as_array_mut().into_iter().flatten() {
                repo["forge"] = serde_json::json!("owner/repo");
            }
            std::fs::write(&list, saved.to_string()).unwrap();
        }
        let config = HostConfig {
            credentials: Credentials::new(dir.join("config")),
            model: Some("gpt-6-luna".into()),
            store: dir.join("unused.db"),
            repos: dir.join("repos"),
            settings: dir.join("models.json"),
            repo_list: list,
            skills: std::env::temp_dir().join("tau-test-skills-none"),
            home: None,
            identity: Identity::default(),
        };
        let agent = Agent::new(gate.clone()).name("coder");
        let (host, events) = Host::with_agent(runtime, agent, store, config);
        // Memory's pass after each run asks the model once more: these
        // scripts answer only the requests they write.
        let host = host.with_plugin_settings(
            tau_memory::NAME,
            serde_json::json!({ "after_each_run": false }),
        );
        // Forecasts start at once: no person's burst of changes to wait
        // out.
        let host = host.with_forecast_wait(Duration::from_millis(1));
        let host = REPOS.iter().fold(host, |host, repo| {
            let project = ProjectRepo::open(
                dir.join(repo).join("project"),
                Identity::default(),
            )
            .map(Project::from)
            .unwrap();
            host.with_repository(
                repo,
                Arc::new(tau_vcs_host::JjRepository::new(
                    project,
                    Identity::default(),
                )),
            )
        });
        (host, events)
    }
}

/// A repository at `checkout` with one commit, imported beside it as
/// `project`.
fn new_project(checkout: &Path) {
    std::fs::create_dir_all(checkout).unwrap();
    git(checkout, &["init", "--quiet"]);
    git(
        checkout,
        &["commit", "--quiet", "--allow-empty", "-m", "first"],
    );
    ProjectRepo::import(
        checkout.to_str().unwrap(),
        checkout.with_file_name("project"),
        Identity::default(),
    )
    .unwrap();
}

/// The computer, its phones and the network between them.
pub struct Net {
    /// The computer's host, to ask whether it is still at work.
    pub host: Arc<Host>,
    /// Keeps the app the windows are in.
    pub app: TestAppContext,
    pub computer: Device,
    pub phones: Vec<Device>,
    pub wire: Rc<RefCell<Wire>>,
    pub gate: Gate,
    disk: Disk,
    /// How many times the computer started again.
    pub restarts: u64,
}

/// The feed's first number for the computer's `start`th start: a new
/// server's numbers come after any the last one sent, as its clock
/// makes them.
fn feed(start: u64) -> Feed<u32> {
    Feed::new(1_000 + start * 1_000_000)
}

impl Net {
    pub fn new(phones: usize, keep: Keep) -> Self {
        let app = TestAppContext::single();
        app.executor().allow_parking();
        app.update(tau_ui_remote::init);
        let gate = Gate::default();
        let disk = Disk::new(keep);
        let wire = Rc::new(RefCell::new(Wire {
            feed: feed(0),
            links: (0..phones).map(|_| Link::new()).collect(),
            next_conn: 0,
        }));
        let (host, computer) = computer(&app, &disk, &gate, &wire);
        let phones = (0..phones)
            .map(|phone| self::phone(&app, &wire, phone))
            .collect();
        let mut net = Self {
            host,
            app,
            computer,
            phones,
            wire,
            gate,
            disk,
            restarts: 0,
        };
        net.settle();
        net
    }

    pub fn device(&mut self, who: Who) -> &mut Device {
        match who {
            Who::Computer => &mut self.computer,
            Who::Phone(phone) => &mut self.phones[phone],
        }
    }

    /// Someone who can act: the computer, or a phone, connected or not;
    /// a phone `phone_weight` of the time.
    pub fn who(&self, tc: &TestCase, phone_weight: f64) -> Who {
        if !tc.draw(gs::weighted_booleans(phone_weight)) {
            return Who::Computer;
        }
        // Connected or not: what it asks goes once it reaches the
        // computer.
        let last = self.phones.len() - 1;
        Who::Phone(tc.draw(gs::integers().min_value(0).max_value(last)))
    }

    /// Runs the computer until its host has nothing left to do: every
    /// run it shows going on waits for the model, and nothing changes a
    /// while.
    pub fn host_settles(&mut self) {
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
            // The host builds its catalog on its own, outside its
            // counted work: the window waits for the models' settings it
            // would build one with now.
            if idle && still == 1 {
                let catalog = self.host.block_on(self.host.catalog());
                if catalog.models.settings != now.0.catalog.models.settings {
                    still = 0;
                    last = None;
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
            }
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
    pub fn deliver(&mut self, phone: usize, down: bool) -> bool {
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
                    link.last_taken = Some((id, body.clone()));
                }
                new
            };
            if !taken {
                return true;
            }
            self.take(body);
        }
        true
    }

    /// The computer takes what a phone sent, as `phone_server` does.
    pub fn take(&mut self, body: Value) {
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

    /// The computer sends the snapshot `phone` waits for.
    pub fn send_snapshot(&mut self, phone: usize) {
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
    pub fn settle(&mut self) {
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
    pub fn say_to(&mut self, who: Who, run: &RunId, text: String) {
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.navigate(Route::Run(run.clone()), cx);
            ws.submit_prompt(text, cx);
        });
        device.cx.run_until_parked();
    }

    pub fn after_action(&mut self, who: Who) {
        if let Who::Computer = who {
            self.host_settles();
        }
    }

    /// tau closes and starts again on what it left on disk: every
    /// phone's connection goes with it, and its feed numbers anew. Runs
    /// it cut off stay cut off; the test answers none of their requests.
    pub fn restart(&mut self) {
        assert_eq!(self.disk.keep, Keep::Disk, "a store in memory is lost");
        let shown = self.computer.runs().len();
        for phone in 0..self.phones.len() {
            self.wire.borrow_mut().drop_phone(phone);
        }
        self.restarts += 1;
        self.wire.borrow_mut().feed = feed(self.restarts);
        self.computer.cx.update(|window, _| window.remove_window());
        self.computer.cx.run_until_parked();
        // The cut runs' requests go unanswered with the host they were
        // made of.
        self.gate = Gate::default();
        let (host, computer) =
            computer(&self.app, &self.disk, &self.gate, &self.wire);
        let old = std::mem::replace(&mut self.host, host);
        self.computer = computer;
        drop(old);
        // The window shows the stored runs once the host has read them.
        let deadline = Instant::now() + SETTLE;
        while self.computer.runs().len() < shown && Instant::now() < deadline {
            self.host_settles();
        }
        self.host_settles();
    }
}

impl Net {
    /// Phone `phone`'s app closes and opens again, numbering its
    /// requests after `after`, as its clock says: what it showed and
    /// what waited in its outbox are gone, as `remote` keeps them only
    /// in memory. It connects again at once.
    pub fn restart_phone(&mut self, phone: usize, after: u64) {
        self.wire.borrow_mut().drop_phone(phone);
        {
            let mut wire = self.wire.borrow_mut();
            let link = &mut wire.links[phone];
            link.outbox = Outbox::new(after);
            link.last = None;
            link.last_taken = None;
        }
        self.phones[phone]
            .cx
            .update(|window, _| window.remove_window());
        self.phones[phone] = self::phone(&self.app, &self.wire, phone);
    }
}

/// The computer's window over a host started on `disk`: what the host
/// applies goes out, as `phone_server` sends it.
fn computer(
    app: &TestAppContext,
    disk: &Disk,
    gate: &Gate,
    wire: &Rc<RefCell<Wire>>,
) -> (Arc<Host>, Device) {
    let (host, events) = disk.start(gate);
    let catalog = host.block_on(host.catalog());
    let mut cx = app.clone();
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", Vec::new(), catalog, window, cx)
    });
    let workspace = window.root(&mut cx).unwrap();
    let mut computer_cx = VisualTestContext::from_window(window.into(), &cx);
    let host = computer_cx.update(|_, cx| host.attach(&workspace, events, cx));
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
    (host, computer)
}

/// Phone `phone`'s window: what the person asks for goes up, as `remote`
/// sends it, now if connected, and kept until taken.
fn phone(
    app: &TestAppContext,
    wire: &Rc<RefCell<Wire>>,
    phone: usize,
) -> Device {
    let mut cx = app.clone();
    let window = cx.add_window(|window, cx| {
        Workspace::new("phone", Vec::new(), Default::default(), window, cx)
    });
    let workspace = window.root(&mut cx).unwrap();
    let mut phone_cx = VisualTestContext::from_window(window.into(), &cx);
    let up = wire.clone();
    phone_cx.update(|_, cx| {
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            if !event.from_phone() {
                return;
            }
            let up_event = PhoneUp::Event(event.clone());
            let Some(body) = remote::to_computer(&up_event) else {
                return;
            };
            let mut wire = up.borrow_mut();
            let link = &mut wire.links[phone];
            let request = link.outbox.push(body);
            if link.conn.is_some() {
                link.up.push_back(request);
            }
        })
        .detach();
    });
    wire.borrow_mut().connect(phone);
    Device {
        workspace,
        cx: phone_cx,
    }
}

/// One of `runs`, which are not empty.
pub fn pick(tc: &TestCase, runs: &[RunView]) -> RunId {
    let n = tc.draw(gs::integers().min_value(0).max_value(runs.len() - 1));
    runs[n].id.clone()
}

/// Fails with the first field `theirs` shows otherwise than `ours`;
/// `who` names whose it is.
pub fn assert_synced(who: &str, ours: &Synced, theirs: &Synced) {
    // A phone is sent the catalog without what computer-only plugins
    // keep (`HostUpdate::for_phones`): the two compare without it.
    let ours = &Synced {
        catalog: ours.catalog.clone().for_phones(),
        ..ours.clone()
    };
    let theirs = &Synced {
        catalog: theirs.catalog.clone().for_phones(),
        ..theirs.clone()
    };
    if ours == theirs {
        return;
    }
    macro_rules! field {
        ($($name:ident),*) => {$(
            assert_eq!(
                theirs.$name, ours.$name,
                "{who} shows another `{}` than the computer",
                stringify!($name)
            );
        )*};
    }
    for (ours, their_run) in ours.runs.iter().zip(&theirs.runs) {
        assert_eq!(their_run, ours, "{who} shows the run otherwise");
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
        query_result,
        ui_state
    );
    unreachable!("Synced compared unequal with every field equal");
}
