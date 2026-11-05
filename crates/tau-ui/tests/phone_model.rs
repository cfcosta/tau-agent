//! The phone's own state (decision 0013) against a model of what the
//! person did with it: which computer it paired with, what it keeps of
//! that pairing, and what it asked that computer for.
//!
//! The phone is the real app's Rust side: a `Workspace` and
//! `remote::connect`, with its credentials in a directory that outlives
//! its restarts and a camera that reads the code the test puts in front
//! of it. Each computer is a real `Workspace` with tau-phones' server,
//! serving over loopback TLS, behind a proxy that stands for the
//! network: the test cuts it, moves the computer to another address, or
//! puts something else at that address. A small plugin of the test's
//! own draws a sidebar entry naming its computer and what it said last,
//! so a view built by one computer and shown for another shows.
//!
//! What the phone keeps (`remote.rs`): one computer's [`Credentials`],
//! as JSON in `remote.json` in the app's directory. Its outbox and the
//! last message it had live in memory only. It keeps one computer: there
//! is no list to switch in, nor a way to forget one; pairing with
//! another replaces it, which a person can do from pairing's first
//! screen or from "Can't reach tau".
//!
//! Only the Android build reaches the camera (`TauScanner`, given here
//! as a closure), the phone's name, the notifications and their taps
//! (`TauNotices`), and the app's directory; the model drives everything
//! `remote.rs` does with them.
//!
//! The rules: pair with either computer, from the screens that offer it
//! (no computer yet, or the paired one not answering), which is how a
//! phone goes from one computer to the other; pair and close before the
//! credentials are kept; the computer revokes the phone, moves to
//! another address, or makes a new certificate; something else answers
//! at its address with a copy of its paired phones (so it would take
//! the phone's token); the network goes and comes back; the app closes
//! and starts again, also while it writes its credentials; the computer
//! says something; the person asks for something.
//!
//! The test stands in for the clock: while it waits for the phone, it
//! wakes the session waiting to try again, as "Try again" does.
//!
//! Checked after every step:
//! - what the phone keeps on disk is the pairing the model has, never a
//!   half-written file;
//! - nothing else at a computer's address took the phone's token;
//! - a request reaches only the computer it was made for, and at most
//!   once; one made while away reaches it once it is back, while the
//!   phone has not paired again nor closed;
//! - a phone that reaches its computer shows what that computer shows,
//!   and draws that computer's views;
//! - a revoked phone says so and offers pairing, and stops knocking;
//! - one that cannot reach its computer says so.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::BTreeSet,
    net::SocketAddr,
    sync::{
        Arc,
        Mutex,
        Once,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use gpui::{Entity, TestAppContext, VisualTestContext};
use hegel::{HealthCheck, TestCase, generators as gs};
use serde_json::json;
use tau_agent::tool::RunId;
use tau_phones::{Act, Phones};
use tau_remote::{
    Address,
    Credentials,
    Fingerprint,
    PairingCode,
    Server,
    ServerConfig,
    ServerEvent,
    ServerHandle,
    reach,
};
use tau_ui::{demo, mirror};
use tau_ui_kit::assets::Icon;
use tau_ui_plugin::{
    Link,
    Manifest,
    NavEntry,
    Registry,
    UiEvent,
    UiPlugin,
    points::{self, AtApp},
};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::Catalog,
    pairing::{PairRequest, PairStep},
    plugins,
    remote::{self, Platform},
    route::Route,
    update::HostUpdate,
};
use tokio::{
    net::{TcpListener, TcpStream},
    runtime::Runtime,
    sync::mpsc::UnboundedReceiver,
    task::AbortHandle,
};

/// How many times the test looks before it gives up on the phone.
const LOOKS: usize = 300;
/// How long it waits between looks.
const LOOK: Duration = Duration::from_millis(10);

/// What the model does, as it does it, with `PHONE_MODEL_TRACE` set.
fn trace(what: impl FnOnce() -> String) {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var_os("PHONE_MODEL_TRACE").is_some()
    });
    static START: std::sync::LazyLock<std::time::Instant> =
        std::sync::LazyLock::new(std::time::Instant::now);
    if *ON {
        eprintln!("phone model {:?}: {}", START.elapsed(), what());
    }
}

/// The test's plugin: a sidebar entry naming the computer that built it
/// and what that computer said last, from its UI state.
struct Tag;

impl UiPlugin for Tag {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        "tag"
    }

    fn declaration(&self) -> tau_ui_plugin::Declaration {
        tau_ui_plugin::Declaration::new()
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().contribute(points::SIDEBAR, |_: &AtApp, view| {
            let who = view.ui_value("who").cloned().unwrap_or_default();
            let said = view.ui_value("said").cloned().unwrap_or_default();
            Some(NavEntry::new(
                format!("{who} said {said}"),
                Icon::Plug,
                Link::page("page"),
            ))
        })
    }
}

fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| plugins::install(Registry::new().with(Tag)));
}

/// The sidebar entries `ws` draws, as text.
fn sidebar(ws: &Workspace) -> Vec<String> {
    ws.contributions_named(points::SIDEBAR, |_| Some(&AtApp))
        .into_iter()
        .map(|(plugin, entry)| format!("{plugin}: {}", entry.label))
        .collect()
}

fn ui(key: &str, value: serde_json::Value) -> HostUpdate {
    HostUpdate::Ui(UiEvent {
        plugin: "tag".into(),
        key: key.into(),
        value: Some(value),
    })
}

/// The phone's network to one computer: a TCP proxy the test cuts,
/// moves, or points at something else.
struct Proxy {
    address: Address,
    /// Where it forwards to: the computer's server, or what stands in
    /// for it.
    upstream: Arc<Mutex<Option<SocketAddr>>>,
    /// The phone's network: shared by every proxy.
    up: Arc<AtomicBool>,
    pipes: Arc<Mutex<Vec<AbortHandle>>>,
    /// Connections the phone opened through it, over its life.
    accepted: Arc<AtomicUsize>,
    listener: AbortHandle,
}

impl Proxy {
    fn start(
        runtime: &Runtime,
        ip: &str,
        upstream: Arc<Mutex<Option<SocketAddr>>>,
        up: Arc<AtomicBool>,
        accepted: Arc<AtomicUsize>,
    ) -> Self {
        let listener = runtime.block_on(TcpListener::bind((ip, 0))).unwrap();
        let address = Address {
            host: ip.into(),
            port: listener.local_addr().unwrap().port(),
        };
        let pipes = Arc::new(Mutex::new(Vec::new()));
        let (to, open, kept, count) = (
            upstream.clone(),
            up.clone(),
            pipes.clone(),
            accepted.clone(),
        );
        let listener = runtime.spawn(async move {
            loop {
                let Ok((phone, _)) = listener.accept().await else {
                    continue;
                };
                count.fetch_add(1, Ordering::SeqCst);
                let target = *to.lock().unwrap();
                let Some(target) =
                    target.filter(|_| open.load(Ordering::SeqCst))
                else {
                    drop(phone);
                    continue;
                };
                let pipe = tokio::spawn(async move {
                    let mut phone = phone;
                    if let Ok(mut computer) = TcpStream::connect(target).await {
                        let _ = tokio::io::copy_bidirectional(
                            &mut phone,
                            &mut computer,
                        )
                        .await;
                    }
                });
                kept.lock().unwrap().push(pipe.abort_handle());
            }
        });
        Self {
            address,
            upstream,
            up,
            pipes,
            accepted,
            listener: listener.abort_handle(),
        }
    }

    /// Drops every connection through it.
    fn cut(&self) {
        for pipe in self.pipes.lock().unwrap().drain(..) {
            pipe.abort();
        }
    }

    /// Listens somewhere else from now on: the old address answers no
    /// more.
    fn moved(&mut self, runtime: &Runtime, ip: &str) {
        self.listener.abort();
        self.cut();
        *self = Self::start(
            runtime,
            ip,
            self.upstream.clone(),
            self.up.clone(),
            self.accepted.clone(),
        );
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.listener.abort();
        self.cut();
    }
}

/// Something else at a computer's address, with a certificate of its
/// own and a copy of the computer's paired phones: it takes a token it
/// is sent.
struct Impostor {
    _handle: ServerHandle,
    events: UnboundedReceiver<ServerEvent>,
    _dir: tempfile::TempDir,
}

struct Computer {
    index: usize,
    workspace: Entity<Workspace>,
    cx: VisualTestContext,
    dir: tempfile::TempDir,
    proxy: Proxy,
    /// Its phones, on the test's runtime.
    phones: Phones,
    runtime: tokio::runtime::Handle,
    /// The phone's requests that reached it, in order.
    asked: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    impostor: Option<Impostor>,
}

impl Computer {
    fn new(
        app: &TestAppContext,
        runtime: &Runtime,
        index: usize,
        up: Arc<AtomicBool>,
    ) -> Self {
        let mut cx = app.clone();
        let runs = if index == 0 {
            demo::history()
        } else {
            Vec::new()
        };
        // The computer lists the test's plugin, so the phone draws it
        // from what the computer builds (ADR 0046).
        let catalog = Catalog {
            plugins: vec![
                serde_json::from_value(json!({
                    "name": "tag",
                    "description": "The phone model's own",
                    "seams": [],
                    "spend": 0.0,
                    "ui": true,
                }))
                .unwrap(),
            ],
            ..Catalog::default()
        };
        let window = cx.add_window(|window, cx| {
            Workspace::new("tau", runs, catalog, window, cx)
        });
        let workspace = window.root(&mut cx).unwrap();
        let mut cx = VisualTestContext::from_window(window.into(), &cx);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("settings.json"),
            r#"{"allow": true, "listen": "127.0.0.1", "port": 0}"#,
        )
        .unwrap();
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen = asked.clone();
        let app = cx.update(|_, cx| {
            cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
                if let WorkspaceEvent::Cancel { run } = event
                    && run.0.starts_with("req-")
                {
                    seen.borrow_mut().push(run.0.to_string());
                }
            })
            .detach();
            mirror::app(&workspace, cx)
        });
        let phones = runtime.block_on(Phones::open(
            dir.path().to_owned(),
            app,
            Arc::new(|| {}),
        ));
        phones.resume();
        let proxy = Proxy::start(
            runtime,
            "127.0.0.1",
            Arc::default(),
            up,
            Arc::default(),
        );
        let mut computer = Self {
            index,
            workspace,
            cx,
            dir,
            proxy,
            phones,
            runtime: runtime.handle().clone(),
            asked,
            impostor: None,
        };
        computer.serving(None);
        computer.workspace.update(&mut computer.cx, |ws, cx| {
            ws.apply(ui("who", json!(format!("computer {index}"))), cx);
            ws.apply(ui("said", json!(0)), cx);
        });
        computer
    }

    /// Waits for its server to listen with a certificate other than
    /// `not`, and forwards the proxy to it.
    fn serving(&mut self, not: Option<Fingerprint>) {
        for _ in 0..LOOKS {
            self.cx.run_until_parked();
            let error = self.phones.error();
            assert!(error.is_none(), "{error:?}");
            let listening = self
                .phones
                .listening()
                .filter(|(_, fingerprint)| Some(*fingerprint) != not);
            if let Some((address, _)) = listening {
                let at = format!("{}:{}", address.host, address.port);
                *self.proxy.upstream.lock().unwrap() =
                    Some(at.parse().unwrap());
                return;
            }
            std::thread::sleep(LOOK);
        }
        panic!("computer {} did not listen", self.index);
    }

    fn fingerprint(&self) -> Fingerprint {
        self.phones
            .listening()
            .map(|(_, fingerprint)| fingerprint)
            .expect("listening")
    }

    fn devices(&self) -> BTreeSet<String> {
        self.phones.paired().iter().map(|d| d.id.clone()).collect()
    }

    /// The paired phones in its file, read without running anything.
    fn devices_on_disk(&self) -> BTreeSet<String> {
        let Ok(text) =
            std::fs::read_to_string(self.dir.path().join("phones.json"))
        else {
            return BTreeSet::new();
        };
        let list: Vec<serde_json::Value> =
            serde_json::from_str(&text).unwrap_or_default();
        list.iter()
            .filter_map(|device| device["id"].as_str().map(str::to_owned))
            .collect()
    }

    /// Makes a new certificate, the old one gone, as phones are turned
    /// off and on again without it.
    fn new_certificate(&mut self) {
        let old = self.fingerprint();
        self.act(Act::Allow { allow: false });
        self.cx.run_until_parked();
        for file in ["cert.der", "key.der"] {
            std::fs::remove_file(self.dir.path().join(file)).unwrap();
        }
        self.act(Act::Allow { allow: true });
        self.serving(Some(old));
        self.proxy.cut();
    }

    /// Asks its phones for `act`, as its Phones page does.
    fn act(&mut self, act: Act) {
        self.runtime.block_on(self.phones.act(act));
        self.cx.run_until_parked();
    }

    /// A pairing code to show the phone, reaching it through the proxy.
    fn code(&mut self) -> String {
        // A phone that paired a moment ago used the code the computer
        // showed: what it shows may go once it hears of it. It is shown
        // again until it stays.
        let mut code = None;
        for _ in 0..LOOKS {
            self.act(Act::ShowCode);
            code = self.phones.code();
            if code.is_some() {
                break;
            }
            std::thread::sleep(LOOK);
        }
        let mut code: PairingCode = code.expect("a code");
        code.address = self.proxy.address.clone();
        code.to_string()
    }
}

/// The phone app, while it runs.
struct Phone {
    workspace: Entity<Workspace>,
    cx: VisualTestContext,
}

/// A pairing the phone keeps, as the model has it.
#[derive(Debug, Clone, PartialEq)]
struct Paired {
    computer: usize,
    device: String,
    address: Address,
    fingerprint: Fingerprint,
}

/// A request the person made on the phone.
#[derive(Debug, Clone)]
struct Request {
    id: String,
    /// The computer the phone was paired with when it was made.
    computer: usize,
    /// The phone's life it was made in: a pairing or a start ends it.
    life: u64,
    delivered: bool,
}

/// What the phone must show once it settles.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expect {
    /// No computer: pairing's first screen.
    Welcome,
    /// Revoked by its computer: pairing's first screen, saying so.
    Revoked(usize),
    /// Reaching its computer: what it shows.
    Connected(usize),
    /// Its computer does not answer: it says so.
    Unreachable,
}

struct Machine {
    computers: Vec<Computer>,
    phone: Option<Phone>,
    phone_dir: tempfile::TempDir,
    /// What the phone's camera reads next.
    code: Arc<Mutex<Option<String>>>,
    up: Arc<AtomicBool>,
    // The model.
    paired: Option<Paired>,
    revoked: BTreeSet<String>,
    requests: Vec<Request>,
    /// Bumped by every pairing and every start.
    life: u64,
    said: Vec<u64>,
    app: TestAppContext,
    // Dropped last: the computers' and proxies' tasks run on it.
    runtime: Runtime,
}

impl Machine {
    fn new(computers: usize) -> Self {
        trace(|| format!("a case with {computers} computers"));
        install();
        let app = TestAppContext::single();
        app.executor().allow_parking();
        app.update(tau_ui_remote::init);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let up = Arc::new(AtomicBool::new(true));
        let computers = (0..computers)
            .map(|index| Computer::new(&app, &runtime, index, up.clone()))
            .collect();
        let mut machine = Self {
            computers,
            phone: None,
            phone_dir: tempfile::tempdir().unwrap(),
            code: Arc::default(),
            up,
            paired: None,
            revoked: BTreeSet::new(),
            requests: Vec::new(),
            life: 0,
            said: vec![0; 2],
            app,
            runtime,
        };
        machine.start_phone();
        machine.settle();
        machine
    }

    fn start_phone(&mut self) {
        let mut cx = self.app.clone();
        let window = cx.add_window(|window, cx| {
            Workspace::new("tau", Vec::new(), Catalog::default(), window, cx)
        });
        let workspace = window.root(&mut cx).unwrap();
        let mut cx = VisualTestContext::from_window(window.into(), &cx);
        let code = self.code.clone();
        let platform = Platform {
            dir: self.phone_dir.path().to_owned(),
            scan: Arc::new(move || Ok(code.lock().unwrap().clone())),
            name: "Test phone".into(),
        };
        cx.update(|_, cx| remote::connect(platform, &workspace, cx));
        self.life += 1;
        self.phone = Some(Phone { workspace, cx });
    }

    /// Closes the app as the system kills it: nothing it had not done
    /// yet happens.
    fn kill_phone(&mut self) {
        trace(|| "the app is killed".into());
        let Some(Phone { workspace, mut cx }) = self.phone.take() else {
            return;
        };
        cx.update(|window, _| window.remove_window());
        drop(workspace);
        // Releases the window's workspace, and with it the remote.
        self.app.update(|_| {});
        trace(|| "the app is gone".into());
    }

    fn phone(&mut self) -> &mut Phone {
        self.phone.as_mut().expect("the app runs")
    }

    fn pick(&self, tc: &TestCase) -> usize {
        tc.draw(
            gs::integers::<usize>()
                .min_value(0)
                .max_value(self.computers.len() - 1),
        )
    }

    /// Whether the phone's pairing can reach its computer, and what it
    /// would hear there.
    fn expect(&self) -> Expect {
        let Some(paired) = &self.paired else {
            return Expect::Welcome;
        };
        let computer = &self.computers[paired.computer];
        let there = self.up.load(Ordering::SeqCst)
            && computer.proxy.address == paired.address
            && computer.impostor.is_none()
            && computer.fingerprint() == paired.fingerprint;
        match (there, self.revoked.contains(&paired.device)) {
            (false, _) => Expect::Unreachable,
            (true, true) => Expect::Revoked(paired.computer),
            (true, false) => Expect::Connected(paired.computer),
        }
    }

    /// Whether the phone shows what `expect` says, now.
    fn shows(&mut self, expect: Expect) -> bool {
        let pending: Vec<String> = self
            .requests
            .iter()
            .filter(|request| !request.delivered && request.life == self.life)
            .map(|request| request.id.clone())
            .collect();
        let phone = self.phone.as_ref().expect("the app runs");
        let (route, alert, pairing, synced, drawn) =
            phone.workspace.read_with(&phone.cx, |ws, _| {
                (
                    ws.route().clone(),
                    ws.alert().map(|(title, _)| title.to_owned()),
                    ws.pairing().clone(),
                    ws.synced(),
                    sidebar(ws),
                )
            });
        match expect {
            Expect::Welcome => route == Route::Pair(PairStep::Welcome),
            Expect::Revoked(_) => {
                route == Route::Pair(PairStep::Welcome)
                    && alert.is_some_and(|title| {
                        title.contains("no longer knows this phone")
                    })
            }
            Expect::Unreachable => route == Route::Pair(PairStep::Unreachable),
            Expect::Connected(c) => {
                let paired = self.paired.as_ref().unwrap();
                let computer = &self.computers[c];
                let (theirs, their_sidebar) =
                    computer.workspace.read_with(&computer.cx, |ws, _| {
                        (ws.synced(), sidebar(ws))
                    });
                let asked = computer.asked.borrow();
                pairing.computer.as_ref().is_some_and(|shown| {
                    shown.address == paired.address
                        && shown.fingerprint == paired.fingerprint
                }) && !matches!(
                    route,
                    Route::Pair(PairStep::Unreachable | PairStep::Welcome)
                ) && pairing.unsent == 0
                    && synced == theirs
                    && drawn == their_sidebar
                    && pending.iter().all(|id| {
                        self.requests
                            .iter()
                            .find(|request| &request.id == id)
                            .is_none_or(|request| request.computer != c)
                            || asked.contains(id)
                    })
            }
        }
    }

    /// Runs the phone and the computers until the phone shows what the
    /// model expects, waking the phone's session as time passing would.
    fn settle(&mut self) {
        let expect = self.expect();
        let started = std::time::Instant::now();
        trace(|| format!("settling to {expect:?}"));
        for look in 0..LOOKS {
            let phone = self.phone();
            phone.workspace.update(&mut phone.cx, |ws, cx| {
                if look % 3 == 0 {
                    cx.emit(WorkspaceEvent::Pair(PairRequest::Retry));
                }
                // As the app does: the views it draws, asked for.
                let _ = sidebar(ws);
                ws.ask_for_views(cx);
            });
            phone.cx.run_until_parked();
            self.no_impostor_took_the_token();
            if self.shows(expect) {
                trace(|| format!("settled in {:?}", started.elapsed()));
                self.settled(expect);
                return;
            }
            std::thread::sleep(LOOK);
        }
        let phone = self.phone.as_ref().unwrap();
        let shown = phone.workspace.read_with(&phone.cx, |ws, _| {
            (
                ws.route().clone(),
                ws.alert().map(|(t, m)| format!("{t}: {m}")),
                ws.pairing().clone(),
                sidebar(ws),
                ws.synced().ui_state,
            )
        });
        panic!(
            "the phone never showed {expect:?}; it shows {shown:#?}; the \
             model has {:?}",
            self.paired
        );
    }

    /// What the model learns once the phone shows `expect`.
    fn settled(&mut self, expect: Expect) {
        match expect {
            Expect::Connected(c) => {
                let asked = self.computers[c].asked.borrow().clone();
                for request in &mut self.requests {
                    if request.computer == c && asked.contains(&request.id) {
                        request.delivered = true;
                    }
                }
            }
            Expect::Revoked(c) => {
                // It offers pairing again, and stops knocking.
                let knocks = |m: &Self| {
                    m.computers[c].proxy.accepted.load(Ordering::SeqCst)
                };
                let before = knocks(self);
                for _ in 0..10 {
                    let phone = self.phone();
                    phone.workspace.update(&mut phone.cx, |_, cx| {
                        cx.emit(WorkspaceEvent::Pair(PairRequest::Retry))
                    });
                    phone.cx.run_until_parked();
                    std::thread::sleep(LOOK);
                }
                assert_eq!(
                    knocks(self),
                    before,
                    "a revoked phone goes on knocking at its computer"
                );
                self.paired = None;
                // The person reads it, and puts it away.
                let phone = self.phone();
                phone
                    .workspace
                    .update(&mut phone.cx, |ws, cx| ws.dismiss_alert(cx));
            }
            Expect::Welcome | Expect::Unreachable => {}
        }
    }

    /// The phone pairs with `c` by its camera, and the person opens tau.
    fn pair_with(&mut self, c: usize) {
        let before = self.computers[c].devices();
        let code = self.computers[c].code();
        *self.code.lock().unwrap() = Some(code);
        let phone = self.phone();
        phone
            .workspace
            .update(&mut phone.cx, |ws, cx| ws.scan_pairing_code(cx));
        for _ in 0..LOOKS {
            let phone = self.phone();
            phone.cx.run_until_parked();
            let paired = phone.workspace.read_with(&phone.cx, |ws, _| {
                ws.route() == &Route::Pair(PairStep::Paired)
            });
            if paired && self.computers[c].devices().len() > before.len() {
                break;
            }
            std::thread::sleep(LOOK);
        }
        let device = self.computers[c]
            .devices()
            .difference(&before)
            .next()
            .cloned()
            .expect("the computer lists the phone it paired");
        let phone = self.phone();
        phone
            .workspace
            .update(&mut phone.cx, |ws, cx| ws.open_tau(cx));
        self.life += 1;
        let computer = &self.computers[c];
        self.paired = Some(Paired {
            computer: c,
            device,
            address: computer.proxy.address.clone(),
            fingerprint: computer.fingerprint(),
        });
    }

    /// Nothing at a computer's address with another certificate took the
    /// phone's token. Looked at while settling too, so a phone that
    /// connects to an impostor says so, rather than settling wrong.
    fn no_impostor_took_the_token(&mut self) {
        for computer in &mut self.computers {
            if let Some(impostor) = &mut computer.impostor {
                while let Ok(event) = impostor.events.try_recv() {
                    assert!(
                        !matches!(
                            event,
                            ServerEvent::NeedSnapshot { .. }
                                | ServerEvent::DevicesChanged(_)
                        ),
                        "the phone sent its token to a computer whose \
                         certificate is not the one it paired with: \
                         {event:?}"
                    );
                }
            }
        }
    }

    /// Whether the person sees a screen that offers pairing.
    fn offers_pairing(&self) -> bool {
        matches!(
            self.expect(),
            Expect::Welcome | Expect::Unreachable | Expect::Revoked(_)
        )
    }

    /// The person asks the computer `computer` for something: here, or
    /// while away. Returns what it asked.
    fn ask(&mut self, computer: usize) -> String {
        let id = format!("req-{}", self.requests.len());
        self.requests.push(Request {
            id: id.clone(),
            computer,
            life: self.life,
            delivered: false,
        });
        let phone = self.phone();
        phone.workspace.update(&mut phone.cx, |_, cx| {
            cx.emit(WorkspaceEvent::Cancel {
                run: RunId(id.clone().into()),
            })
        });
        id
    }

    /// The phone's network goes, or comes back.
    fn network(&mut self, up: bool) {
        self.up.store(up, Ordering::SeqCst);
        if !up {
            for computer in &self.computers {
                computer.proxy.cut();
            }
        }
    }

    fn restart(&mut self) {
        self.kill_phone();
        self.start_phone();
    }
}

#[hegel::state_machine]
impl Machine {
    /// The person pairs with a computer, from a screen that offers it: no
    /// computer yet, or the paired one does not answer, which is how a
    /// phone goes from one computer to another, or back to the same.
    #[rule(weight = 3)]
    fn pair(&mut self, tc: TestCase) {
        trace(|| "pair".into());
        let c = self.pick(&tc);
        tc.assume(
            self.offers_pairing()
                && self.up.load(Ordering::SeqCst)
                && self.computers[c].impostor.is_none(),
        );
        self.pair_with(c);
    }

    /// The phone pairs, and closes once the computer gave it its token,
    /// before it kept it.
    #[rule]
    fn pair_and_close_before_keeping(&mut self, tc: TestCase) {
        trace(|| "pair_and_close_before_keeping".into());
        let c = self.pick(&tc);
        tc.assume(
            self.offers_pairing()
                && self.up.load(Ordering::SeqCst)
                && self.computers[c].impostor.is_none(),
        );
        let before = self.computers[c].devices_on_disk();
        let code = self.computers[c].code();
        *self.code.lock().unwrap() = Some(code);
        let phone = self.phone();
        phone
            .workspace
            .update(&mut phone.cx, |ws, cx| ws.scan_pairing_code(cx));
        // Only the network runs: the app does not get to the news.
        let mut given = false;
        for _ in 0..LOOKS {
            if self.computers[c].devices_on_disk().len() > before.len() {
                given = true;
                break;
            }
            std::thread::sleep(LOOK);
        }
        assert!(given, "the computer never paired the phone");
        self.restart();
    }

    #[rule]
    fn computer_revokes_the_phone(&mut self, tc: TestCase) {
        trace(|| "computer_revokes_the_phone".into());
        let Some(paired) = self.paired.clone() else {
            tc.reject()
        };
        tc.assume(!self.revoked.contains(&paired.device));
        let computer = &mut self.computers[paired.computer];
        computer.act(Act::Revoke {
            id: paired.device.clone(),
        });
        // Revoked once the computer saved it.
        for _ in 0..LOOKS {
            computer.cx.run_until_parked();
            if !computer.devices().contains(&paired.device) {
                break;
            }
            std::thread::sleep(LOOK);
        }
        assert!(!computer.devices().contains(&paired.device));
        self.revoked.insert(paired.device);
    }

    /// The computer is at another address from now on: another port, or
    /// another IP.
    #[rule(weight = 2)]
    fn computer_moves(&mut self, tc: TestCase) {
        trace(|| "computer_moves".into());
        let c = self.pick(&tc);
        let ip: &str =
            tc.draw(gs::sampled_from(vec!["127.0.0.1", "127.0.0.2"]));
        let runtime = &self.runtime;
        self.computers[c].proxy.moved(runtime, ip);
    }

    /// The computer makes a new certificate: its old one is gone.
    #[rule(weight = 2)]
    fn computer_changes_certificate(&mut self, tc: TestCase) {
        trace(|| "computer_changes_certificate".into());
        let c = self.pick(&tc);
        self.computers[c].new_certificate();
    }

    /// Something else answers at the computer's address, with its own
    /// certificate and the computer's paired phones.
    #[rule]
    fn impostor_at_the_address(&mut self, tc: TestCase) {
        trace(|| "impostor_at_the_address".into());
        let c = self.pick(&tc);
        tc.assume(self.computers[c].impostor.is_none());
        let dir = tempfile::tempdir().unwrap();
        let phones = self.computers[c].dir.path().join("phones.json");
        if phones.exists() {
            std::fs::copy(phones, dir.path().join("phones.json")).unwrap();
        }
        let (handle, events) = self
            .runtime
            .block_on(Server::start(ServerConfig {
                listen: "127.0.0.1:0".parse().unwrap(),
                dir: dir.path().to_owned(),
                host: "impostor".into(),
                clock: std::sync::Arc::new(tau_remote::SystemClock),
            }))
            .unwrap();
        let computer = &mut self.computers[c];
        *computer.proxy.upstream.lock().unwrap() = Some(handle.local_addr());
        computer.proxy.cut();
        computer.impostor = Some(Impostor {
            _handle: handle,
            events,
            _dir: dir,
        });
    }

    #[rule]
    fn impostor_goes(&mut self, tc: TestCase) {
        trace(|| "impostor_goes".into());
        let c = self.pick(&tc);
        let computer = &mut self.computers[c];
        let Some(impostor) = computer.impostor.take() else {
            tc.reject()
        };
        drop(impostor);
        computer.serving(None);
        computer.proxy.cut();
    }

    #[rule(weight = 2)]
    fn network_goes(&mut self, _tc: TestCase) {
        trace(|| "network_goes".into());
        self.network(false);
    }

    #[rule]
    fn network_comes_back(&mut self, _tc: TestCase) {
        trace(|| "network_comes_back".into());
        self.network(true);
    }

    #[rule]
    fn app_restarts(&mut self, _tc: TestCase) {
        trace(|| "app_restarts".into());
        self.restart();
    }

    /// The app is killed while it writes its credentials, and starts
    /// again: the file is what a reader saw at a drawn moment of the
    /// phone's own writes, as the kill would leave it.
    #[rule]
    fn app_killed_while_keeping(&mut self, tc: TestCase) {
        trace(|| "app_killed_while_keeping".into());
        tc.assume(self.paired.is_some());
        let at: usize = tc.draw(gs::integers().min_value(0).max_value(300));
        self.kill_phone();
        let dir = self.phone_dir.path().to_owned();
        let kept = remote::load(&dir).expect("the paired phone kept it");
        let writing = Arc::new(AtomicBool::new(true));
        let writer = {
            let (dir, writing) = (dir.clone(), writing.clone());
            std::thread::spawn(move || {
                while writing.load(Ordering::SeqCst) {
                    remote::store(&dir, Some(&kept)).unwrap();
                }
            })
        };
        let file = dir.join("remote.json");
        let mut seen = Vec::new();
        for _ in 0..=at {
            seen = std::fs::read(&file).unwrap_or_default();
        }
        writing.store(false, Ordering::SeqCst);
        writer.join().unwrap();
        std::fs::write(&file, seen).unwrap();
        self.start_phone();
    }

    /// A computer says something: everything it shows, and its view.
    #[rule]
    fn computer_says(&mut self, tc: TestCase) {
        trace(|| "computer_says".into());
        let c = self.pick(&tc);
        self.said[c] += 1;
        let said = self.said[c];
        let computer = &mut self.computers[c];
        computer.workspace.update(&mut computer.cx, |ws, cx| {
            ws.apply(ui("said", json!(said)), cx)
        });
        computer.cx.run_until_parked();
    }

    /// The person asks for something on the phone: here, or while away.
    #[rule(weight = 3)]
    fn person_asks(&mut self, tc: TestCase) {
        trace(|| "person_asks".into());
        let Some(paired) = self.paired.clone() else {
            tc.reject()
        };
        tc.assume(!matches!(self.expect(), Expect::Revoked(_)));
        self.ask(paired.computer);
    }

    /// The phone's credentials on disk are whole, or not there: checked
    /// first, as a torn file leaves the phone unpaired, not slow.
    #[invariant(always_run)]
    fn no_half_written_credentials(&mut self, _tc: TestCase) {
        let file = self.phone_dir.path().join("remote.json");
        if file.exists() {
            let text = std::fs::read_to_string(&file).unwrap();
            assert!(
                serde_json::from_str::<Credentials>(&text).is_ok(),
                "the phone kept credentials half-written: {text:?}"
            );
        }
    }

    /// The phone shows what the model expects, once it settles.
    #[invariant(always_run)]
    fn the_phone_settles(&mut self, _tc: TestCase) {
        self.settle();
    }

    /// What the phone keeps on disk is the pairing the model has.
    #[invariant(always_run)]
    fn the_phone_keeps_its_pairing(&mut self, _tc: TestCase) {
        let kept = remote::load(self.phone_dir.path()).map(|credentials| {
            (
                credentials.address,
                credentials.fingerprint,
                credentials.device,
            )
        });
        let model = self.paired.as_ref().map(|paired| {
            (
                paired.address.clone(),
                paired.fingerprint,
                paired.device.clone(),
            )
        });
        assert_eq!(kept, model, "what the phone keeps is not its pairing");
    }

    /// Nothing at a computer's address with another certificate took the
    /// phone's token: the phone never sent it.
    #[invariant(always_run)]
    fn no_token_to_another_certificate(&mut self, _tc: TestCase) {
        self.no_impostor_took_the_token();
    }

    /// A request reaches only the computer it was made for, at most once.
    #[invariant(always_run)]
    fn requests_reach_their_computer_once(&mut self, _tc: TestCase) {
        for computer in &self.computers {
            let asked = computer.asked.borrow();
            let mut seen = BTreeSet::new();
            for id in asked.iter() {
                assert!(
                    seen.insert(id),
                    "{id} reached computer {} twice",
                    computer.index
                );
                let request = self
                    .requests
                    .iter()
                    .find(|request| &request.id == id)
                    .expect("a request the person made");
                assert_eq!(
                    request.computer, computer.index,
                    "{id} was made for computer {}, and reached computer {}",
                    request.computer, computer.index
                );
            }
        }
    }

    /// The paired computer's way there, in words, is what its address
    /// says.
    #[invariant(always_run)]
    fn the_way_there_follows_the_address(&mut self, _tc: TestCase) {
        let phone = self.phone.as_ref().unwrap();
        let computer = phone
            .workspace
            .read_with(&phone.cx, |ws, _| ws.pairing().computer.clone());
        if let Some(computer) = computer {
            assert_eq!(computer.via.as_deref(), reach(&computer.address.host));
        }
    }
}

fn run(tc: TestCase, steps: i64) {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let earlier = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            trace(|| format!("failed: {info}"));
            earlier(info)
        }))
    });
    // Two computers: a phone that never goes to the second is a case too.
    let machine = Machine::new(2);
    hegel::stateful::machine(machine).steps(steps).run(tc);
}

// Real servers and a real phone per case: generating is slow by nature.
#[hegel::test(test_cases = 40, suppress_health_check = [HealthCheck::TooSlow])]
fn a_phone_keeps_its_computer_through_restarts_revokes_and_switches(
    tc: TestCase,
) {
    run(tc, 20);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn a_phone_keeps_its_computer_through_restarts_revokes_and_switches_nightly(
    tc: TestCase,
) {
    run(tc, 40);
}

// What the model leaves for later: each test is what the decided answer
// looks like, and the ignore says what brings it.

/// The phone's requests live in memory: one made while away is lost if
/// the app closes before the computer is back.
#[test]
#[ignore = "fixed by the CRDT sync of ADR 0047: the phone persists its \
            replica, so what it asked while away outlives a restart"]
fn a_request_made_while_away_survives_a_restart() {
    let mut m = Machine::new(1);
    m.pair_with(0);
    m.settle();
    m.network(false);
    m.settle();
    let id = m.ask(0);
    m.restart();
    m.network(true);
    m.settle();
    assert!(m.computers[0].asked.borrow().contains(&id));
}

/// Paired again with the same computer, the phone drops what it asked of
/// it before: the new pairing is a new device to the computer, which
/// takes each device's requests once, not each phone's.
#[test]
#[ignore = "fixed by the CRDT sync of ADR 0047: a stable phone identity, \
            apart from the device token, keeps a re-pair from duplicating"]
fn a_request_made_before_pairing_again_reaches_the_same_computer() {
    let mut m = Machine::new(1);
    m.pair_with(0);
    m.settle();
    m.computers[0].new_certificate();
    m.settle();
    let id = m.ask(0);
    m.pair_with(0);
    m.settle();
    assert!(m.computers[0].asked.borrow().contains(&id));
}

/// A paired computer whose certificate changed looks to the phone like
/// one that does not answer: it says it cannot reach it, and keeps
/// trying, though trying again never works.
#[test]
#[ignore = "Should a phone whose computer's certificate changed say so? \
            (a) no: it cannot reach it, and keeps trying, as now; (b) yes: \
            say the certificate changed, stop trying, and offer pairing \
            again; (c) say so, but keep trying, in case the old \
            certificate comes back"]
fn a_changed_certificate_is_said_so() {
    let mut m = Machine::new(1);
    m.pair_with(0);
    m.settle();
    m.computers[0].new_certificate();
    m.settle();
    let phone = m.phone();
    let said = phone.workspace.read_with(&phone.cx, |ws, _| {
        format!("{:?} {:?}", ws.alert(), ws.pairing().progress)
    });
    assert!(said.contains("certificate"), "{said}");
}

/// The phone goes from one computer to the other, as a person does when
/// the first stops answering: it shows the second's views, not the
/// first's (an example the model reaches too, here every run).
#[test]
fn a_phone_that_switches_shows_the_new_computers_views() {
    let mut m = Machine::new(2);
    m.pair_with(0);
    m.settle();
    let runtime = &m.runtime;
    m.computers[0].proxy.moved(runtime, "127.0.0.1");
    m.settle();
    m.pair_with(1);
    m.settle();
    let phone = m.phone();
    let drawn = phone.workspace.read_with(&phone.cx, |ws, _| sidebar(ws));
    assert_eq!(drawn, vec!["tag: \"computer 1\" said 0".to_owned()]);
}
