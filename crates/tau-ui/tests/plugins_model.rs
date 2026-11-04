//! Plugins composed by the kernel (ADR 0045), drawn on the computer and
//! on phones that draw what the computer builds (ADR 0046), against what
//! a person who used any of them must see on all of them.
//!
//! The process installs every plugin tau has, with its host half, and
//! three probes of the test's own: each draws a banner over a run with
//! buttons, a section, an input and a link, a status line, a sidebar
//! entry, a repository menu entry and a page, all from its state in the
//! run and its UI state, and logs every action it is handed. Two of the
//! probes contribute to the banner with the same order, so the registry
//! decides between them; each probe fills a slot of its own.
//!
//! The computer is the real `Workspace` over that registry. The host's
//! part this test needs is the glue `tau-ui`'s host and phone server
//! put between a workspace and the network, done here in a few lines:
//! a plugin's record is stored and folded, a UI event is applied, saved
//! as JSON and echoed, and a phone's asks, drops and calls go to the
//! computer's `ask_views`, `drop_views` and `answer_call`. Phones reach
//! it through the real feed, outbox and encoding, in process: what the
//! network does is the test's to pick, message by message, including
//! dropping a phone and bringing it back, and restarting the computer.
//!
//! Checked after every step: the computer shows the UI state the host
//! saved; what the host stored, folded again, and that state, read back
//! from JSON, are what the computer shows (a restart here would change
//! nothing); each plugin's part of a point is the same alone as among
//! the others; what the computer builds for its own window is what it
//! draws; a plugin's sidebar entry opens that plugin's page; and the
//! probes' buttons are drawn whole, at their own size, inside the
//! window. Each click, on any device, reaches the plugin that drew what
//! was clicked. Once every message has arrived, every phone shows what
//! the computer shows, and draws the trees the computer builds for it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
    sync::{Arc, LazyLock, Mutex, Once},
};

use gpui::{
    Bounds,
    Entity,
    Modifiers,
    Pixels,
    TestAppContext,
    VisualTestContext,
    px,
    size,
};
use hegel::{HealthCheck, TestCase, generators as gs};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_remote::{
    Down,
    Up,
    feed::{Feed, Joined},
    outbox::{self, Outbox},
};
use tau_ui::{hosted, phone_server};
use tau_ui_kit::assets::Icon;
use tau_ui_plugin::{
    Fold,
    HostCx,
    HostHalf,
    Link,
    Manifest,
    NavEntry,
    Page,
    PluginInfo,
    Point,
    PointCx,
    RunCtx,
    RunCx,
    Services,
    UiPlugin,
    UiState,
    ViewCx,
    points::{self, AtApp, AtRepo, AtRun},
    slots::{Fills, PICKS, Slot, chosen},
    tree::{Kind, Node, draft_key, open_key},
    wire::{Built, View, ViewKey, Wired},
};
use tau_ui_remote::{
    Workspace,
    WorkspaceEvent,
    catalog::{self, Catalog, Repo},
    pairing::PairingUpdate,
    phones::PhoneUp,
    plugins,
    remote,
    route::Route,
    update::HostUpdate,
    view::RunView,
    workspace::Synced,
};

/// The repository the run is in, and the run: its main chat.
const REPO: &str = "repo";
const MAIN: &str = "main";

fn main_run() -> RunId {
    RunId(MAIN.into())
}

/// The probes' names, and the order each contributes its banner at: the
/// first two tie, so the registry's order puts one above the other.
const PROBES: [&str; 3] = ["probe-a", "probe-b", "probe-c"];
const ORDERS: [i32; 3] = [0, 0, -1];

/// Every action a probe was handed, on whichever device it was drawn:
/// the probe, the action, and its arguments.
static ACTED: Mutex<Vec<(String, String, Value)>> = Mutex::new(Vec::new());

fn acted() -> Vec<(String, String, Value)> {
    ACTED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// A slot's filler, as the probes fill theirs: who it is.
trait Probed: Send + Sync {
    fn who(&self) -> &'static str;
}

struct Filler(&'static str);

impl Probed for Filler {
    fn who(&self) -> &'static str {
        self.0
    }
}

/// `test.alpha`'s declared order: the rival, registered after the
/// probes, fills it first.
const ALPHA_ORDER: [&str; 2] = ["rival", "probe-a"];

/// The slots the probes fill: `probe-a` the first, which the rival can
/// fill too, and `probe-b` the second.
const SLOTS: [Slot<dyn Probed>; 2] =
    [Slot::new("test.alpha"), Slot::new("test.beta")];

/// What a probe folds from the records it publishes: how many times its
/// button was pressed in the run.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Bumps {
    n: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Bump {
    by: u32,
}

impl Fold for Bumps {
    type Record = Bump;

    fn apply(&mut self, record: Bump, _run: &mut dyn RunCx) {
        self.n += record.by;
    }
}

/// A plugin of the test's own: see the module docs.
struct Probe<const N: usize>;

/// The probe's draft, as its UI state has it.
fn draft<P: UiPlugin>(view: &ViewCx<'_, P>) -> String {
    view.ui_value(&draft_key("draft"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

impl<const N: usize> Probe<N> {
    /// Its banner over a run: what it counted and holds, a button in a
    /// row and one in the column, a section, and an input.
    fn banner(view: &ViewCx<'_, Self>) -> Node {
        let name = PROBES[N];
        let n = view.state.map_or(0, |state| state.n);
        Node::stack([
            Node::text(format!("{name}: {n} bumps, draft {:?}", draft(view))),
            Node::row([
                button(&format!("{name} bump"), "bump", json!({ "by": 1 })),
                Node::Link {
                    label: format!("{name} page"),
                    to: Link::page("page"),
                },
            ]),
            button(&format!("{name} mark"), "mark", json!({ "probe": name })),
            Node::Section {
                id: "details".into(),
                title: format!("{name} details"),
                open: false,
                children: vec![Node::text(format!("{name} is open"))],
            },
            Node::Input {
                id: "draft".into(),
                placeholder: Some(format!("{name} draft")),
                multiline: false,
                submit: None,
                keep: false,
                secret: false,
                focus: None,
            },
        ])
    }
}

fn button(label: &str, action: &str, args: Value) -> Node {
    Node::Button {
        label: label.to_owned(),
        action: action.to_owned(),
        args,
        kind: Kind::Secondary,
        icon: None,
    }
}

impl<const N: usize> UiPlugin for Probe<N> {
    type State = Bumps;
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        PROBES[N]
    }

    fn on_action(
        &self,
        action: &str,
        args: &Value,
        view: &mut ViewCx<'_, Self>,
    ) {
        ACTED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((PROBES[N].to_owned(), action.to_owned(), args.clone()));
        if action == "bump"
            && let Some(run) = view.run
        {
            let by = args["by"].as_u64().unwrap_or(1) as u32;
            view.handle.record(&run.id, Bump { by });
        }
    }

    fn manifest(&self) -> Manifest<Self> {
        let name = PROBES[N];
        Manifest::new()
            .contribute_at(points::RUN_BANNER, ORDERS[N], |_, view| {
                Some(Self::banner(view))
            })
            .status(|state: &Bumps| Some(format!("{} bumps", state.n)))
            .contribute(points::SIDEBAR, move |_, _| {
                Some(NavEntry::new(
                    format!("{name} entry"),
                    Icon::Plug,
                    Link::page("page"),
                ))
            })
            .contribute(points::REPO_MENU, move |at: &AtRepo, _| {
                Some(Node::Link {
                    label: format!("{name} notes"),
                    to: Link::page("page").param("repo", at.repo.clone()),
                })
            })
            .page(Page::new("page", move |view| {
                Node::stack([
                    Node::text(format!("{name} page, draft {:?}", draft(view))),
                    button(&format!("{name} page mark"), "mark", json!({})),
                ])
            }))
    }
}

/// What a probe keeps on the host: nothing.
#[derive(Default)]
struct Kept;

struct ProbeHost<const N: usize>;

impl<const N: usize> HostHalf for ProbeHost<N> {
    type Plugin = Probe<N>;
    type Host = Kept;

    async fn agent_plugins(
        &self,
        _host: &Kept,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn tau_agent::plugin::Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _host: &Kept,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description: "A probe of the plugins model".into(),
            ..Default::default()
        }
    }

    fn fill(&self, _host: &Kept, fills: &mut Fills) {
        if let Some(slot) = SLOTS.get(N) {
            fills.fill(slot, Arc::new(Filler(PROBES[N])) as Arc<dyn Probed>);
        }
    }
}

/// Installs tau's plugins, each with its host half, and the probes: the
/// process's registry, once.
fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| {
        plugins::install(
            tau::plugins()
                .with(Probe::<0>)
                .host(ProbeHost::<0>)
                .with(Probe::<1>)
                .host(ProbeHost::<1>)
                .with(Probe::<2>)
                .host(ProbeHost::<2>)
                // Registered last, and first in the slot's order: the
                // order decides, not the registration.
                .with(Rival)
                .host(RivalHost)
                .slot_order(&SLOTS[0], &ALPHA_ORDER),
        )
    });
}

/// Every plugin's host state, made once as a host makes them: what
/// fills the slots.
struct Hosting {
    hosted: Vec<hosted::Hosted>,
    _cx: HostCx,
    _runtime: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

static HOSTING: LazyLock<Hosting> = LazyLock::new(|| {
    install();
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let cx = HostCx::new(
        store,
        runtime.handle().clone(),
        Services::default()
            .with(tau_ui_plugin::ConfigDir(dir.path().join("config"))),
        dir.path().to_owned(),
        Vec::new(),
        Arc::new(|_| {}),
    );
    let hosted = runtime.block_on(hosted::host_all(&cx));
    Hosting {
        hosted,
        _cx: cx,
        _runtime: runtime,
        _dir: dir,
    }
});

/// Who fills each slot the kernel and the probes declare: the forge by
/// its id, a probe's by the probe, version control by being filled.
fn slot_table(fills: &Fills) -> BTreeMap<&'static str, Option<String>> {
    let mut table = BTreeMap::new();
    table.insert(
        tau_ui_plugin::vcs::VCS.name(),
        fills
            .get(&tau_ui_plugin::vcs::VCS)
            .map(|_| "version control".to_owned()),
    );
    table.insert(
        tau_ui_plugin::forge::FORGE.name(),
        fills
            .get(&tau_ui_plugin::forge::FORGE)
            .map(|forge| forge.id().to_owned()),
    );
    for slot in &SLOTS {
        table.insert(
            slot.name(),
            fills.get(slot).map(|filler| filler.who().to_owned()),
        );
    }
    table
}

/// The catalog the computer starts with: one repository with its main
/// chat, and every plugin of the registry, with its UI.
fn catalog() -> Catalog {
    Catalog {
        plugins: plugins::registry()
            .plugins()
            .map(|plugin| catalog::PluginInfo {
                name: plugin.name().to_owned(),
                description: String::new(),
                seams: Vec::new(),
                spend: 0.0,
                page: None,
                group: Default::default(),
                note: None,
                entries: Vec::new(),
                settings: plugin.has_settings(),
                ui: true,
                rewrites_keep_transcript: plugin.rewrites_keep_transcript(),
            })
            .collect(),
        repos: vec![Repo {
            name: REPO.into(),
            main: Some(main_run()),
            ..Default::default()
        }],
        slots: {
            let fills = hosted::fills(&HOSTING.hosted);
            fills
                .slots()
                .into_iter()
                .map(|slot| catalog::SlotInfo {
                    name: slot.to_owned(),
                    candidates: fills.candidates(slot),
                })
                .collect()
        },
        ..Default::default()
    }
}

/// The runs as the host's history makes them: the main chat, with
/// every record stored with it folded in order, and chats forked from
/// it, whose rows lie under the repository's menu.
fn runs_from(records: &[(RunId, String, Value)]) -> Vec<RunView> {
    let mut run =
        RunView::new(main_run(), "main", "coder", "gpt-5.5").in_repo(REPO);
    for (id, plugin, body) in records {
        if *id == run.id {
            run.fold(plugin, body);
        }
    }
    let chats = (0..CHATS).map(|n| {
        RunView::new(
            RunId(format!("chat-{n}").into()),
            "a chat",
            "coder",
            "gpt-5.5",
        )
        .in_repo(REPO)
        .with_origin(tau_ui_remote::view::Origin::Fork {
            from: main_run(),
            turn: 0,
        })
    });
    std::iter::once(run).chain(chats).collect()
}

/// How many chats the repository has besides its main.
const CHATS: usize = 6;

/// A phone's end of the network.
struct PhoneLink {
    /// Its connection, while it has one.
    conn: Option<u32>,
    /// What the computer sent that has not arrived.
    down: VecDeque<Down>,
    /// What the phone sent on its connection that has not arrived.
    up: VecDeque<Up>,
    /// What the phone asked that the computer has not taken.
    outbox: Outbox,
    /// The last request the computer took from it.
    taken: Option<u64>,
    /// The last numbered message it applied.
    last: Option<u64>,
    /// It needs a snapshot the computer has not sent.
    waiting: bool,
}

/// The network between the computer and its phones.
struct Wire {
    feed: Feed<u32>,
    links: Vec<PhoneLink>,
    next_conn: u32,
}

impl Wire {
    /// Sends what the computer applied, as the phone server does: kept
    /// and numbered, or only to the phones there now.
    fn send(&mut self, update: &HostUpdate) {
        let Some(body) = phone_server::to_phones(update) else {
            return;
        };
        let (down, to) = if phone_server::transient(update) {
            self.feed.transient(body)
        } else {
            self.feed.broadcast(body)
        };
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
        let pending: Vec<Up> = link.outbox.pending().collect();
        link.up.extend(pending);
    }

    /// `phone`'s connection drops: what was on its way is lost, both
    /// ways; its outbox keeps what was not taken.
    fn drop_phone(&mut self, phone: usize) -> bool {
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
    fn read<R>(&mut self, f: impl FnOnce(&Workspace) -> R) -> R {
        self.workspace.read_with(&self.cx, |ws, _| f(ws))
    }

    fn synced(&mut self) -> Synced {
        self.read(Workspace::synced)
    }

    /// Where the run is drawn, its banners and plugins' lines.
    fn at_run(&mut self) -> AtRun {
        self.read(|ws| AtRun {
            run: ws.run(&main_run()).expect("the run is shown").info(),
        })
    }

    /// Where `selector` is drawn now. The window draws first: what the
    /// host applied since its last frame moved things, and a click is
    /// dispatched on a fresh frame.
    fn bounds(&mut self, selector: &str) -> Option<Bounds<Pixels>> {
        self.cx.update(|window, cx| window.draw(cx).clear(cx));
        self.cx.run_until_parked();
        // gpui keeps the selectors it looks up.
        self.cx
            .debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
    }

    fn click(&mut self, bounds: Bounds<Pixels>) {
        self.cx.simulate_click(bounds.center(), Modifiers::none());
        self.cx.run_until_parked();
    }

    fn open(&mut self, route: Route) {
        self.workspace
            .update(&mut self.cx, |ws, cx| ws.navigate(route, cx));
        self.cx.run_until_parked();
    }
}

/// Who acts: the computer, or a phone.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Who {
    Computer,
    Phone(usize),
}

/// What reached the host, in order: the computer's own requests, and
/// each phone's as the computer takes them.
type Inbox = Rc<RefCell<VecDeque<(Option<usize>, WorkspaceEvent)>>>;

/// What the host keeps: the records stored with the run, and the UI
/// state as it saved it.
#[derive(Default)]
struct Stored {
    records: Vec<(RunId, String, Value)>,
    ui: UiState,
    /// `ui`, as written to `ui.json`.
    saved: String,
    /// What fills the slots, with the picks the host took, as `tau-ui`'s
    /// host keeps them.
    slots: Fills,
}

impl Stored {
    fn new() -> Self {
        let ui = UiState::default();
        Self {
            records: Vec::new(),
            saved: serde_json::to_string(&ui).unwrap(),
            slots: slots_from(&ui),
            ui,
        }
    }
}

/// The slots as a host starting with `ui` fills them.
fn slots_from(ui: &UiState) -> Fills {
    let mut fills = hosted::fills(&HOSTING.hosted);
    fills.picks_from(ui);
    fills
}

/// Who must fill `test.alpha`: the person's pick, else the first in its
/// declared order, of those in `hosted`.
fn alpha_filler(hosted: &[&str], ui: &UiState) -> Option<String> {
    let candidates: Vec<String> = ALPHA_ORDER
        .iter()
        .filter(|name| hosted.contains(name))
        .map(|name| (*name).to_owned())
        .collect();
    let pick = ui.get(PICKS, SLOTS[0].name()).and_then(Value::as_str);
    chosen(&candidates, pick).map(str::to_owned)
}

struct Tau {
    app: TestAppContext,
    computer: Device,
    phones: Vec<Device>,
    wire: Rc<RefCell<Wire>>,
    inbox: Inbox,
    host: Stored,
    /// The computer applied something that may change a phone's view.
    refresh: Rc<RefCell<bool>>,
    /// The plugins each phone's calls named, in order.
    calls: Vec<Rc<RefCell<Vec<String>>>>,
    /// The bumps clicked, by probe: what the run must count once every
    /// click arrived.
    bumped: BTreeMap<&'static str, u32>,
    /// How many times the computer restarted, for its feed's numbers.
    restarts: u64,
}

/// The points the model looks at, each drawn at the run, the app or
/// the repository.
#[derive(Debug, Clone, Copy)]
enum Place {
    Banner,
    Status,
    Sidebar,
    RepoMenu,
}

const PLACES: [Place; 4] = [
    Place::Banner,
    Place::Status,
    Place::Sidebar,
    Place::RepoMenu,
];

impl Tau {
    fn new(phones: usize) -> Self {
        install();
        let app = TestAppContext::single();
        app.update(tau_ui_remote::init);
        let wire = Rc::new(RefCell::new(Wire {
            feed: Feed::new(0),
            links: (0..phones)
                .map(|_| PhoneLink {
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
        let inbox = Inbox::default();
        let refresh = Rc::new(RefCell::new(false));
        let host = Stored::new();
        let computer = computer(&app, &host, &wire, &inbox, &refresh);

        let mut calls = Vec::new();
        let phones = (0..phones)
            .map(|phone| {
                let mut cx = app.clone();
                let window = cx.add_window(|window, cx| {
                    Workspace::new(
                        "phone",
                        Vec::new(),
                        Catalog::default(),
                        window,
                        cx,
                    )
                });
                let workspace = window.root(&mut cx).unwrap();
                let mut phone_cx =
                    VisualTestContext::from_window(window.into(), &cx);
                phone_cx.simulate_resize(size(px(400.), px(844.)));
                workspace.update(&mut phone_cx, |ws, _| ws.draw_remote_views());
                let called = Rc::new(RefCell::new(Vec::new()));
                calls.push(called.clone());
                // What the person asks for goes up, as `remote` sends it.
                let up = wire.clone();
                phone_cx.update(|_, cx| {
                    cx.subscribe(
                        &workspace,
                        move |_, event: &WorkspaceEvent, _| {
                            if let WorkspaceEvent::PluginCall {
                                plugin, ..
                            } = event
                            {
                                called.borrow_mut().push(plugin.clone());
                            }
                            if !event.from_phone() {
                                return;
                            }
                            let Some(body) = remote::to_computer(
                                &PhoneUp::Event(event.clone()),
                            ) else {
                                return;
                            };
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
            app,
            computer,
            phones,
            wire,
            inbox,
            host,
            refresh,
            calls,
            bumped: BTreeMap::new(),
            restarts: 0,
        };
        tau.settle();
        for phone in 0..tau.phones.len() {
            tau.phones[phone].open(Route::Run(main_run()));
        }
        tau.settle();
        tau
    }

    fn device(&mut self, who: Who) -> &mut Device {
        match who {
            Who::Computer => &mut self.computer,
            Who::Phone(phone) => &mut self.phones[phone],
        }
    }

    fn who(&self, tc: &TestCase) -> Who {
        if tc.draw(gs::booleans()) {
            return Who::Computer;
        }
        let last = self.phones.len() - 1;
        Who::Phone(tc.draw(gs::integers().min_value(0).max_value(last)))
    }

    /// The host does what reached it, as `tau-ui`'s host and phone
    /// server do, until nothing is left: records are stored and folded,
    /// UI events saved and echoed, phones' asks answered, and the views
    /// phones draw built again after a change.
    fn host_settles(&mut self) {
        loop {
            self.computer.cx.run_until_parked();
            let next = self.inbox.borrow_mut().pop_front();
            let Some((from, event)) = next else {
                if !std::mem::take(&mut *self.refresh.borrow_mut()) {
                    return;
                }
                let changed = self
                    .computer
                    .workspace
                    .update(&mut self.computer.cx, |ws, _| ws.refresh_views());
                if let Some(changed) = changed {
                    self.wire.borrow_mut().send(&changed);
                }
                continue;
            };
            let computer = &mut self.computer;
            match event {
                WorkspaceEvent::Ui(event) => {
                    self.host.ui.apply(&event);
                    // A pick holds from now on, as `Host::set_ui` does.
                    if event.plugin == PICKS {
                        self.host.slots.pick(
                            &event.key,
                            event.value.as_ref().and_then(Value::as_str),
                        );
                    }
                    self.host.saved =
                        serde_json::to_string(&self.host.ui).unwrap();
                    computer.workspace.update(&mut computer.cx, |ws, cx| {
                        ws.apply(HostUpdate::Ui(event), cx)
                    });
                }
                WorkspaceEvent::PluginRecord { run, plugin, body } => {
                    self.host.records.push((
                        run.clone(),
                        plugin.clone(),
                        body.clone(),
                    ));
                    computer.workspace.update(&mut computer.cx, |ws, cx| {
                        ws.apply(
                            HostUpdate::PluginRecord { run, plugin, body },
                            cx,
                        )
                    });
                }
                WorkspaceEvent::PluginCall {
                    id,
                    plugin,
                    origin,
                    layout,
                    call,
                } => {
                    computer.workspace.update(&mut computer.cx, |ws, cx| {
                        ws.answer_call(id, &plugin, &origin, layout, &call, cx)
                    });
                }
                WorkspaceEvent::AskViews(keys) => {
                    let phone = from.expect("only a phone asks for views");
                    let answers =
                        computer.workspace.update(&mut computer.cx, |ws, _| {
                            ws.ask_views(&format!("phone-{phone}"), keys)
                        });
                    self.wire.borrow_mut().send(&answers);
                }
                WorkspaceEvent::DropViews(keys) => {
                    let phone = from.expect("only a phone drops views");
                    computer.workspace.update(&mut computer.cx, |ws, _| {
                        ws.drop_views(&format!("phone-{phone}"), &keys)
                    });
                }
                _ => {}
            }
        }
    }

    /// `phone` connects again, and hears so as its remote tells it.
    fn connect(&mut self, phone: usize) {
        self.wire.borrow_mut().connect(phone);
        let device = &mut self.phones[phone];
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.update_pairing(
                PairingUpdate::Connected(tau_ui::demo::computer()),
                cx,
            )
        });
        device.cx.run_until_parked();
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
            if seq.is_some() {
                self.wire.borrow_mut().links[phone].last = seq;
            }
        } else {
            let next = self.wire.borrow_mut().links[phone].up.pop_front();
            let Some(Up::Up { id, body }) = next else {
                return false;
            };
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
            if let Some(PhoneUp::Event(event)) = phone_server::from_phone(body)
            {
                self.inbox.borrow_mut().push_back((Some(phone), event));
            }
            self.host_settles();
        }
        true
    }

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
        for _ in 0..64 {
            let mut moved = false;
            for phone in 0..self.phones.len() {
                let (conn, waiting) = {
                    let wire = self.wire.borrow();
                    let link = &wire.links[phone];
                    (link.conn, link.waiting)
                };
                if conn.is_none() {
                    self.connect(phone);
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
                // Each frame draws the places the model looks at, and
                // what it drew without a view, it asks for.
                for place in PLACES {
                    Self::drawn(&mut self.phones[phone], place);
                }
                let device = &mut self.phones[phone];
                device
                    .workspace
                    .update(&mut device.cx, |ws, cx| ws.ask_for_views(cx));
                device.cx.run_until_parked();
                moved |= !self.wire.borrow().links[phone].up.is_empty();
            }
            if !moved {
                assert!(self.wire.borrow().settled());
                return;
            }
            self.host_settles();
        }
        panic!("the computer and its phones never settled");
    }

    /// What `device` draws at `place`, by plugin, as wire JSON.
    fn drawn(device: &mut Device, place: Place) -> Vec<(String, Value)> {
        let at = device.at_run();
        let wired = |outs: Vec<(&'static str, Value)>| {
            outs.into_iter()
                .map(|(plugin, out)| (plugin.to_owned(), out))
                .collect::<Vec<_>>()
        };
        device.read(|ws| {
            fn of<Cx: PointCx, Out: Wired>(
                ws: &Workspace,
                point: Point<Cx, Out>,
                at: &Cx,
            ) -> Vec<(&'static str, Value)> {
                ws.contributions_named(point, |_| Some(at))
                    .into_iter()
                    .filter_map(|(plugin, out)| Some((plugin, out.to_wire()?)))
                    .collect()
            }
            wired(match place {
                Place::Banner => of(ws, points::RUN_BANNER, &at),
                Place::Status => of(ws, points::STATUS, &at),
                Place::Sidebar => of(ws, points::SIDEBAR, &AtApp),
                Place::RepoMenu => {
                    of(ws, points::REPO_MENU, &AtRepo { repo: REPO.into() })
                }
            })
        })
    }

    /// What `device` draws at `place` from `plugin` alone.
    fn drawn_alone(
        device: &mut Device,
        place: Place,
        plugin: &str,
    ) -> Vec<Value> {
        let at = device.at_run();
        device.read(|ws| {
            fn of<Cx: PointCx, Out: Wired>(
                ws: &Workspace,
                point: Point<Cx, Out>,
                at: &Cx,
                plugin: &str,
            ) -> Vec<Value> {
                ws.contributions_named(point, |name| {
                    (name == plugin).then_some(at)
                })
                .into_iter()
                .filter_map(|(_, out)| out.to_wire())
                .collect()
            }
            match place {
                Place::Banner => of(ws, points::RUN_BANNER, &at, plugin),
                Place::Status => of(ws, points::STATUS, &at, plugin),
                Place::Sidebar => of(ws, points::SIDEBAR, &AtApp, plugin),
                Place::RepoMenu => of(
                    ws,
                    points::REPO_MENU,
                    &AtRepo { repo: REPO.into() },
                    plugin,
                ),
            }
        })
    }

    /// The view the computer builds for `place` in `layout`.
    fn built(
        &mut self,
        place: Place,
        layout: tau_ui_plugin::wire::Layout,
    ) -> Vec<(String, Value)> {
        let at = match place {
            Place::Banner | Place::Status => self.computer.at_run().wire(),
            Place::Sidebar => AtApp.wire(),
            Place::RepoMenu => AtRepo { repo: REPO.into() }.wire(),
        }
        .expect("the place crosses");
        let point = match place {
            Place::Banner => points::RUN_BANNER.name,
            Place::Status => points::STATUS.name,
            Place::Sidebar => points::SIDEBAR.name,
            Place::RepoMenu => points::REPO_MENU.name,
        };
        let key = ViewKey {
            view: View::Point {
                point: point.to_owned(),
                at,
            },
            layout,
        };
        match self.computer.read(|ws| ws.build_view(&key)) {
            Built::Outs { outs } => {
                outs.into_iter().map(|out| (out.plugin, out.out)).collect()
            }
            other => panic!("{place:?} built as {other:?}"),
        }
    }

    /// The buttons `who` draws over the run, with the plugin whose tree
    /// has each, where only one plugin's has that label.
    fn buttons(&mut self, who: Who) -> Vec<(String, String)> {
        let banners = Self::drawn(self.device(who), Place::Banner);
        let mut found: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (plugin, tree) in &banners {
            labels(tree, &mut |label| {
                found
                    .entry(label.to_owned())
                    .or_default()
                    .push(plugin.clone())
            });
        }
        found
            .into_iter()
            .filter(|(label, by)| !label.is_empty() && by.len() == 1)
            .map(|(label, by)| (label, by[0].clone()))
            .collect()
    }

    /// The computer starts again: from what the host stored and the UI
    /// state it saved, with every phone's connection gone.
    fn restart_computer(&mut self) {
        self.host_settles();
        let before = self.computer.synced();
        for phone in 0..self.phones.len() {
            self.wire.borrow_mut().drop_phone(phone);
        }
        self.restarts += 1;
        // Its first message's number is new, as a new server's is.
        self.wire.borrow_mut().feed = Feed::new(self.restarts * 1_000_000);
        self.computer.cx.update(|window, _| window.remove_window());
        self.computer = computer(
            &self.app,
            &self.host,
            &self.wire,
            &self.inbox,
            &self.refresh,
        );
        let after = self.computer.synced();
        assert_synced("the computer after a restart", &before, &after);
        let saved: UiState = serde_json::from_str(&self.host.saved).unwrap();
        let slots = slots_from(&saved);
        assert_eq!(
            format!("{slots:?}"),
            format!("{:?}", self.host.slots),
            "the slots are filled otherwise after a restart"
        );
        self.host.slots = slots;
    }
}

/// The computer's window, as the host attaches it: the runs as stored,
/// the UI state as saved, and what it applies sent to phones.
fn computer(
    app: &TestAppContext,
    host: &Stored,
    wire: &Rc<RefCell<Wire>>,
    inbox: &Inbox,
    refresh: &Rc<RefCell<bool>>,
) -> Device {
    let mut cx = app.clone();
    let runs = runs_from(&host.records);
    let window = cx.add_window(|window, cx| {
        Workspace::new("tau", runs, catalog(), window, cx)
    });
    let workspace = window.root(&mut cx).unwrap();
    let mut computer_cx = VisualTestContext::from_window(window.into(), &cx);
    computer_cx.simulate_resize(size(px(1440.), px(900.)));
    let saved: UiState = serde_json::from_str(&host.saved).unwrap();
    workspace.update(&mut computer_cx, |ws, cx| {
        ws.apply(HostUpdate::UiState(saved), cx);
        ws.navigate(Route::Run(main_run()), cx);
        ws.set_mirrored(true);
    });
    let (out, mine, changed) = (wire.clone(), inbox.clone(), refresh.clone());
    computer_cx.update(|_, cx| {
        cx.subscribe(&workspace, move |_, update: &HostUpdate, _| {
            out.borrow_mut().send(update);
            if !matches!(update, HostUpdate::Views { .. }) {
                *changed.borrow_mut() = true;
            }
        })
        .detach();
        cx.subscribe(&workspace, move |_, event: &WorkspaceEvent, _| {
            mine.borrow_mut().push_back((None, event.clone()));
        })
        .detach();
    });
    computer_cx.run_until_parked();
    Device {
        workspace,
        cx: computer_cx,
    }
}

/// Calls `found` with the label of each button in `tree`'s JSON.
fn labels(tree: &Value, found: &mut impl FnMut(&str)) {
    match tree {
        Value::Object(map) => {
            if map.get("piece").and_then(Value::as_str) == Some("button")
                && let Some(label) = map.get("label").and_then(Value::as_str)
            {
                found(label);
            }
            for value in map.values() {
                labels(value, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                labels(item, found);
            }
        }
        _ => {}
    }
}

/// Fails with the first field `theirs` shows otherwise than `ours`.
fn assert_synced(who: &str, ours: &Synced, theirs: &Synced) {
    if ours == theirs {
        return;
    }
    assert_eq!(theirs.ui_state, ours.ui_state, "{who}: the UI state");
    for (ours, their) in ours.runs.iter().zip(&theirs.runs) {
        assert_eq!(their, ours, "{who}: the run");
    }
    assert_eq!(theirs.runs.len(), ours.runs.len(), "{who}: the runs");
    assert_eq!(theirs.catalog, ours.catalog, "{who}: the catalog");
    assert_eq!(theirs, ours, "{who}");
}

#[hegel::state_machine]
impl Tau {
    /// A probe's button over the run is pressed, on the computer or a
    /// phone: the probe that drew it, and only it, is handed the action.
    #[rule(weight = 3)]
    fn press_bump(&mut self, tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().max_value(PROBES.len() - 1));
        let who = self.who(&tc);
        let probe = PROBES[n];
        let device = self.device(who);
        let Some(bounds) = device.bounds(&format!("button:{probe} bump"))
        else {
            // A phone draws a banner once the computer sent it.
            tc.event("the button was not drawn yet");
            return;
        };
        let before = acted().len();
        device.click(bounds);
        *self.bumped.entry(probe).or_default() += 1;
        match who {
            Who::Computer => {
                let acted = acted();
                assert_eq!(
                    acted[before..]
                        .iter()
                        .map(|(plugin, action, _)| (
                            plugin.as_str(),
                            action.as_str()
                        ))
                        .collect::<Vec<_>>(),
                    [(probe, "bump")],
                    "the click reached another plugin"
                );
            }
            Who::Phone(phone) => {
                let called = self.calls[phone].borrow().last().cloned();
                assert_eq!(called.as_deref(), Some(probe), "the phone's call");
            }
        }
        self.host_settles();
    }

    /// Any button drawn over the run, one plugin's alone, is pressed:
    /// what it asks is that plugin's.
    #[rule(weight = 2)]
    fn press_any(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let buttons = self.buttons(who);
        if buttons.is_empty() {
            tc.event("no button was drawn yet");
            return;
        }
        let (label, plugin) = tc.draw(gs::sampled_from(buttons));
        let device = self.device(who);
        let Some(bounds) = device.bounds(&format!("button:{label}")) else {
            tc.event("the button was laid out of sight");
            return;
        };
        let before = acted().len();
        let asked = self.inbox.borrow().len();
        let device = self.device(who);
        device.click(bounds);
        if label.ends_with(" bump") {
            let probe = PROBES.iter().find(|probe| **probe == plugin);
            if let Some(probe) = probe {
                *self.bumped.entry(probe).or_default() += 1;
            }
        }
        // The computer hands it to the plugin, and what the plugin asks
        // of the host names it.
        if who == Who::Computer {
            for (by, _, _) in &acted()[before..] {
                assert_eq!(*by, plugin, "{label} reached another plugin");
            }
            for (_, event) in self.inbox.borrow().iter().skip(asked) {
                let named = match event {
                    WorkspaceEvent::PluginRecord { plugin, .. }
                    | WorkspaceEvent::PluginAct { plugin, .. }
                    | WorkspaceEvent::PluginSettings { plugin, .. }
                    | WorkspaceEvent::PluginFetch { plugin, .. } => plugin,
                    WorkspaceEvent::Ui(event) => &event.plugin,
                    _ => continue,
                };
                assert_eq!(*named, plugin, "{label} asked as another plugin");
            }
        }
        if let Who::Phone(phone) = who {
            let called = self.calls[phone].borrow().last().cloned();
            assert_eq!(called.as_deref(), Some(plugin.as_str()), "{label}");
        }
        self.host_settles();
    }

    /// A probe's section opens or closes, on the computer or a phone.
    #[rule(weight = 2)]
    fn toggle_section(&mut self, tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().max_value(PROBES.len() - 1));
        let who = self.who(&tc);
        let probe = PROBES[n];
        let open = self.device(who).read(|ws| {
            ws.synced()
                .ui_state
                .get(probe, &open_key("details"))
                .cloned()
        });
        let device = self.device(who);
        let Some(bounds) = device.bounds(&format!("section:{probe} details"))
        else {
            tc.event("the section was not drawn yet");
            return;
        };
        device.click(bounds);
        let now = device.read(|ws| {
            ws.synced()
                .ui_state
                .get(probe, &open_key("details"))
                .cloned()
        });
        assert_ne!(now, open, "the click on {probe}'s section changed nothing");
        self.host_settles();
    }

    /// A probe's draft is typed, on the computer or a phone.
    #[rule(weight = 2)]
    fn type_draft(&mut self, tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().max_value(PROBES.len() - 1));
        let who = self.who(&tc);
        let text: String =
            tc.draw(gs::from_regex("[a-c]{0,3}").fullmatch(true));
        let value = (!text.is_empty()).then(|| json!(text));
        let device = self.device(who);
        device.workspace.update(&mut device.cx, |ws, cx| {
            ws.set_plugin_ui(PROBES[n], draft_key("draft"), value, cx)
        });
        device.cx.run_until_parked();
        self.host_settles();
    }

    /// A goal is set on the run, through tau-goal's records: a plugin
    /// tau has draws its banner, with its own buttons.
    #[rule]
    fn set_goal(&mut self, tc: TestCase) {
        let goal: String = tc.draw(gs::sampled_from(vec![
            "it ships".to_owned(),
            "the tests pass".to_owned(),
        ]));
        let body = serde_json::to_value(tau_goal::Record::Set {
            goal,
            continuations: 3,
            budget: 2.0,
        })
        .unwrap();
        self.inbox.borrow_mut().push_back((
            None,
            WorkspaceEvent::PluginRecord {
                run: main_run(),
                plugin: tau_goal::NAME.into(),
                body,
            },
        ));
        self.host_settles();
    }

    /// An entry in the repository's menu, over the sidebar's rows, is
    /// clicked on the computer: a probe's opens its page, and tau's own
    /// "New run" a new run; nothing under the menu takes the click.
    #[rule]
    fn repo_menu_entry(&mut self, tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().max_value(PROBES.len()));
        let (selector, label) = match PROBES.get(n) {
            Some(probe) => (format!("link:{probe} notes"), *probe),
            None => ("menu-new".to_owned(), "tau's New run"),
        };
        let device = &mut self.computer;
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.toggle_repo_menu(REPO, cx));
        device.cx.run_until_parked();
        let Some(bounds) = device.bounds(&selector) else {
            device
                .workspace
                .update(&mut device.cx, |ws, cx| ws.close_repo_menu(cx));
            tc.event("the menu entry was not drawn");
            return;
        };
        device.click(bounds);
        let route = device.read(|ws| ws.route().clone());
        let opened = match PROBES.get(n) {
            Some(probe) => matches!(
                &route,
                Route::Plugin { plugin, page, .. }
                    if plugin == probe && page == "page"
            ),
            None => route == Route::NewRun,
        };
        assert!(opened, "{label}'s menu entry opened {route:?}");
        device
            .workspace
            .update(&mut device.cx, |ws, cx| ws.close_repo_menu(cx));
        device.open(Route::Run(main_run()));
        self.host_settles();
    }

    /// A probe's page is opened, on the computer or a phone, and its
    /// button pressed; then the run again.
    #[rule]
    fn open_page(&mut self, tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().max_value(PROBES.len() - 1));
        let who = self.who(&tc);
        let probe = PROBES[n];
        let device = self.device(who);
        device.open(Route::Plugin {
            plugin: probe.into(),
            page: "page".into(),
            params: BTreeMap::new(),
        });
        if let Who::Phone(phone) = who {
            // The page is the computer's to build.
            self.host_settles();
            while self.deliver(phone, false) {}
            while self.deliver(phone, true) {}
        }
        let device = self.device(who);
        if let Some(bounds) =
            device.bounds(&format!("button:{probe} page mark"))
        {
            let before = acted().len();
            device.click(bounds);
            self.host_settles();
            if who == Who::Computer {
                let acted = acted();
                assert_eq!(
                    acted[before..]
                        .iter()
                        .map(|(plugin, action, _)| (
                            plugin.as_str(),
                            action.as_str()
                        ))
                        .collect::<Vec<_>>(),
                    [(probe, "mark")],
                );
            }
            if let Who::Phone(phone) = who {
                let called = self.calls[phone].borrow().last().cloned();
                assert_eq!(called.as_deref(), Some(probe), "the page's call");
            }
        } else {
            tc.event("the page was not drawn yet");
        }
        self.device(who).open(Route::Run(main_run()));
        self.host_settles();
    }

    /// The registry's plugins in another order, some left out, fill the
    /// slots: each slot that one plugin of them can fill is filled by
    /// it, whatever the order.
    #[rule]
    fn register_in_order(&mut self, tc: TestCase) {
        let hosted = &HOSTING.hosted;
        let mut picked: Vec<hosted::Hosted> = hosted
            .iter()
            .filter(|_| tc.draw(gs::weighted_booleans(0.8)))
            .cloned()
            .collect();
        // A random order: each picks a place among those left.
        let mut ordered = Vec::new();
        while !picked.is_empty() {
            let at =
                tc.draw(gs::integers::<usize>().max_value(picked.len() - 1));
            ordered.push(picked.remove(at));
        }
        // What each fills alone. Only `test.alpha` has two who can
        // fill it; together they fill each other slot with the one who
        // can, and it with the person's pick or the first in its order.
        let mut alone: BTreeMap<&'static str, Option<String>> =
            slot_table(&Fills::default());
        for hosted in &ordered {
            let mut fills = Fills::default();
            hosted.plugin.fill(&hosted.state, &mut fills);
            for (slot, filler) in slot_table(&fills) {
                if let Some(filler) = filler
                    && slot != SLOTS[0].name()
                {
                    let before = alone.insert(slot, Some(filler));
                    assert_eq!(before, Some(None), "{slot} has two fillers");
                }
            }
        }
        let names: Vec<&str> =
            ordered.iter().map(|hosted| hosted.plugin.name()).collect();
        alone.insert(SLOTS[0].name(), alpha_filler(&names, &self.host.ui));
        // All of them fill every slot: tau's own fillers, tau-vcs-host
        // and tau-github-host, are among them.
        let all = slot_table(&hosted::fills(hosted));
        assert!(all.values().all(Option::is_some), "{all:?}");
        let mut fills = hosted::fills(&ordered);
        fills.picks_from(&self.host.ui);
        assert_eq!(
            slot_table(&fills),
            alone,
            "the slots are filled otherwise in the order {names:?}"
        );
    }

    /// The person picks another plugin to fill `test.alpha` on the
    /// Plugins screen, on the computer or a phone: that window shows it
    /// at once.
    #[rule(weight = 2)]
    fn pick_filler(&mut self, tc: TestCase) {
        let who = self.who(&tc);
        let slot = SLOTS[0].name();
        let device = self.device(who);
        let now = device.read(|ws| ws.slot_filler(slot).map(str::to_owned));
        let plugin = ALPHA_ORDER
            .into_iter()
            .find(|plugin| Some(*plugin) != now.as_deref())
            .expect("two can fill it");
        device.open(Route::Plugins);
        let Some(bounds) = device.bounds(&format!("slot:{slot}:{plugin}"))
        else {
            tc.event("the slot's row was not drawn yet");
            self.device(who).open(Route::Run(main_run()));
            return;
        };
        device.click(bounds);
        let filler = device.read(|ws| ws.slot_filler(slot).map(str::to_owned));
        assert_eq!(filler.as_deref(), Some(plugin), "the pick shows here");
        device.open(Route::Run(main_run()));
        self.host_settles();
    }

    /// One message crosses the network, one way or the other.
    #[rule(weight = 4)]
    fn one_arrives(&mut self, tc: TestCase) {
        let phone =
            tc.draw(gs::integers::<usize>().max_value(self.phones.len() - 1));
        let down: bool = tc.draw(gs::booleans());
        if !self.deliver(phone, down) {
            tc.event("nothing was on its way");
        }
    }

    /// A phone's connection drops: what was on its way is lost.
    #[rule]
    fn phone_drops(&mut self, tc: TestCase) {
        let phone =
            tc.draw(gs::integers::<usize>().max_value(self.phones.len() - 1));
        if !self.wire.borrow_mut().drop_phone(phone) {
            tc.event("the phone was not connected");
            return;
        }
        // Its only connection closed: the server says it left, and the
        // phone server drops what the computer built for it.
        let computer = &mut self.computer;
        computer.workspace.update(&mut computer.cx, |ws, _| {
            ws.forget_phone(&format!("phone-{phone}"))
        });
    }

    /// A phone that dropped connects again, from the last message it
    /// had.
    #[rule]
    fn phone_comes_back(&mut self, tc: TestCase) {
        let phone =
            tc.draw(gs::integers::<usize>().max_value(self.phones.len() - 1));
        if self.wire.borrow().links[phone].conn.is_some() {
            tc.event("the phone was connected");
            return;
        }
        self.connect(phone);
    }

    /// The computer answers a phone that needs everything.
    #[rule]
    fn snapshot_sent(&mut self, tc: TestCase) {
        let phone =
            tc.draw(gs::integers::<usize>().max_value(self.phones.len() - 1));
        if !self.wire.borrow().links[phone].waiting {
            tc.event("the phone needed no snapshot");
            return;
        }
        self.send_snapshot(phone);
    }

    /// tau on the computer quits and starts again.
    #[rule]
    fn restart(&mut self, _tc: TestCase) {
        self.restart_computer();
    }

    /// Everything arrives: every phone shows what the computer shows,
    /// draws what the computer builds for it, and the run counts every
    /// click.
    #[rule(weight = 2)]
    fn all_arrive(&mut self, _tc: TestCase) {
        self.settle();
        let computer = self.computer.synced();
        for phone in 0..self.phones.len() {
            let theirs = self.phones[phone].synced();
            assert_synced(&format!("phone {phone}"), &computer, &theirs);
            let slot = SLOTS[0].name();
            let (ours, their) = (
                self.computer
                    .read(|ws| ws.slot_filler(slot).map(str::to_owned)),
                self.phones[phone]
                    .read(|ws| ws.slot_filler(slot).map(str::to_owned)),
            );
            assert_eq!(their, ours, "phone {phone}'s filler of {slot}");
            let layout = self.phones[phone].read(Workspace::view_layout);
            for place in PLACES {
                let built = self.built(place, layout);
                let drawn = Self::drawn(&mut self.phones[phone], place);
                assert_eq!(
                    drawn, built,
                    "phone {phone} draws {place:?} otherwise than the \
                     computer builds it"
                );
            }
        }
        let at = self.computer.at_run();
        let statuses = self.computer.read(|ws| {
            ws.contributions_named(points::STATUS, |_| Some(&at))
                .into_iter()
                .map(|(plugin, status)| (plugin.to_owned(), status.state))
                .collect::<BTreeMap<_, _>>()
        });
        for probe in PROBES {
            let bumped = self.bumped.get(probe).copied().unwrap_or(0);
            assert_eq!(
                statuses.get(probe).map(String::as_str),
                Some(format!("{bumped} bumps").as_str()),
                "{probe} counts its clicks"
            );
        }
    }

    /// The computer shows the UI state the host saved, and what the host
    /// stored and saved is what it shows: a restart now would change
    /// nothing.
    /// `test.alpha` is filled by the person's pick, else by the first in
    /// its declared order, whatever the registration order: on the host
    /// and as the computer shows it.
    #[invariant(always_run)]
    fn the_pick_or_the_order_fills_a_slot(&mut self, _tc: TestCase) {
        let slot = SLOTS[0].name();
        let want = alpha_filler(&ALPHA_ORDER, &self.host.ui);
        assert_eq!(self.host.slots.filler(slot), want, "the host's filler");
        let shown = self
            .computer
            .read(|ws| ws.slot_filler(slot).map(str::to_owned));
        assert_eq!(shown, want, "the computer's Plugins screen");
    }

    /// The computer builds views only for the phones connected now.
    #[invariant(always_run)]
    fn views_are_built_for_phones_there(&mut self, _tc: TestCase) {
        let there: Vec<String> = (0..self.phones.len())
            .filter(|phone| self.wire.borrow().links[*phone].conn.is_some())
            .map(|phone| format!("phone-{phone}"))
            .collect();
        for (key, by) in self.computer.read(Workspace::asked_views) {
            for device in by {
                assert!(
                    there.contains(&device),
                    "{device} is gone, and {key:?} is still built for it"
                );
            }
        }
    }

    #[invariant(always_run)]
    fn a_restart_changes_nothing(&mut self, _tc: TestCase) {
        let shown = self.computer.synced();
        let saved: UiState = serde_json::from_str(&self.host.saved).unwrap();
        assert_eq!(saved, self.host.ui, "what the host saved reads back");
        assert_eq!(shown.ui_state, saved, "the computer's UI state");
        assert_eq!(
            shown.runs,
            runs_from(&self.host.records),
            "the run, folded again from what was stored"
        );
        let json = serde_json::to_value(&shown).unwrap();
        assert_eq!(serde_json::from_value::<Synced>(json).unwrap(), shown);
    }

    /// Each plugin's part of a place is what it draws there alone; what
    /// the computer builds for its own window is what it draws; and each
    /// sidebar entry opens its own plugin's page.
    #[invariant(always_run)]
    fn plugins_compose(&mut self, _tc: TestCase) {
        let layout = self.computer.read(Workspace::view_layout);
        for place in PLACES {
            let whole = Self::drawn(&mut self.computer, place);
            for plugin in plugins::registry().plugins() {
                let part: Vec<Value> = whole
                    .iter()
                    .filter(|(by, _)| by == plugin.name())
                    .map(|(_, out)| out.clone())
                    .collect();
                assert_eq!(
                    part,
                    Self::drawn_alone(&mut self.computer, place, plugin.name()),
                    "{}'s part of {place:?}",
                    plugin.name()
                );
            }
            assert_eq!(
                self.built(place, layout),
                whole,
                "the computer builds {place:?} otherwise than it draws it"
            );
        }
        let mut devices: Vec<&mut Device> = vec![&mut self.computer];
        devices.extend(self.phones.iter_mut());
        for device in devices {
            let entries: Vec<(&'static str, NavEntry)> = device.read(|ws| {
                ws.contributions_named(points::SIDEBAR, |_| Some(&AtApp))
            });
            for (plugin, entry) in entries {
                let route = device.read(|ws| ws.link_route(&entry.to, None));
                assert!(
                    matches!(
                        &route,
                        Some(Route::Plugin { plugin: to, .. }) if to == plugin
                    ),
                    "{plugin}'s entry {:?} opens {route:?}",
                    entry.label
                );
            }
        }
    }

    /// The probes' buttons over the run, where a window draws them, lie
    /// inside it, and are as wide as what they say, not as the column
    /// they are in.
    #[invariant(always_run)]
    fn buttons_are_drawn_whole(&mut self, _tc: TestCase) {
        let mut devices: Vec<(&str, &mut Device)> =
            vec![("the computer", &mut self.computer)];
        devices.extend(self.phones.iter_mut().map(|phone| ("a phone", phone)));
        for (who, device) in devices {
            if device.read(|ws| ws.route().clone()) != Route::Run(main_run()) {
                continue;
            }
            let window = device.read(|ws| f32::from(ws.screen_width()));
            for probe in PROBES {
                let (Some(mark), Some(section)) = (
                    device.bounds(&format!("button:{probe} mark")),
                    device.bounds(&format!("section:{probe} details")),
                ) else {
                    continue;
                };
                assert!(
                    f32::from(mark.left()) >= 0.
                        && f32::from(mark.right()) <= window + 0.5,
                    "on {who}, {probe}'s button runs out of the window: \
                     {mark:?} in {window}"
                );
                assert!(
                    mark.size.width < section.size.width * 0.5,
                    "on {who}, {probe}'s button is stretched across its \
                     column: {mark:?}, the section's {section:?}"
                );
            }
        }
    }
}

#[hegel::test(test_cases = 12, suppress_health_check = [HealthCheck::TooSlow])]
fn plugins_build_the_same_screen_on_every_device(tc: TestCase) {
    let phones = tc.draw(gs::integers().min_value(1).max_value(2));
    hegel::stateful::machine(Tau::new(phones)).steps(20).run(tc);
}

#[hegel::test(
    profile = "nightly_slow",
    suppress_health_check = [HealthCheck::TooSlow]
)]
#[ignore = "nightly"]
fn plugins_build_the_same_screen_on_every_device_nightly(tc: TestCase) {
    let phones = tc.draw(gs::integers().min_value(1).max_value(2));
    hegel::stateful::machine(Tau::new(phones)).steps(40).run(tc);
}

/// A plugin that can fill `probe-a`'s slot too, registered after it and
/// first in the slot's order.
struct Rival;

impl UiPlugin for Rival {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = ();

    fn name(&self) -> &'static str {
        "rival"
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new()
    }
}

struct RivalHost;

impl HostHalf for RivalHost {
    type Plugin = Rival;
    type Host = Kept;

    async fn agent_plugins(
        &self,
        _host: &Kept,
        _run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn tau_agent::plugin::Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(
        &self,
        _host: &Kept,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo::default()
    }

    fn fill(&self, _host: &Kept, fills: &mut Fills) {
        fills.fill(&SLOTS[0], Arc::new(Filler("rival")) as Arc<dyn Probed>);
    }
}

/// Two plugins that can both fill a slot: the slot's declared order
/// picks the first, not the order they were registered in, until the
/// person picks the other, which then fills it in any order (ADR 0045).
#[test]
fn a_slot_two_plugins_can_fill_does_not_follow_the_order() {
    let hosting = &*HOSTING;
    let named = |name: &str| {
        hosting
            .hosted
            .iter()
            .find(|hosted| hosted.plugin.name() == name)
            .unwrap()
            .clone()
    };
    let (probe, rival) = (named(PROBES[0]), named("rival"));
    let filler = |hosted: &[hosted::Hosted], pick: Option<&str>| {
        let mut fills = hosted::fills(hosted);
        fills.pick(SLOTS[0].name(), pick);
        slot_table(&fills)[SLOTS[0].name()].clone()
    };
    for pick in [None, Some(PROBES[0]), Some("rival")] {
        let want = pick.unwrap_or("rival");
        assert_eq!(
            filler(&[probe.clone(), rival.clone()], pick).as_deref(),
            Some(want)
        );
        assert_eq!(
            filler(&[rival.clone(), probe.clone()], pick).as_deref(),
            Some(want)
        );
    }
}
