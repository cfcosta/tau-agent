//! tau-mcp's UI: the page's actions change the settings as they say and
//! refuse what they should; an approval holds for the entry the page
//! showed and no other; the host keeps one plugin per repository and
//! builds it again when its servers change; the page, the card and the
//! sidebar draw what the host says; and the UI keeps to the design
//! language.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::Path,
    rc::Rc,
    sync::Arc,
};

use gpui::{
    App,
    AppContext as _,
    Context,
    Entity,
    IntoElement,
    ParentElement as _,
    Render,
    Styled as _,
    TestAppContext,
    VisualTestContext,
    Window,
    div,
    px,
    size,
};
use hegel::{
    TestCase,
    generators::{self as gs, Generator as _},
};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_mcp::{
    NAME,
    config::{
        Exposure,
        HttpConfig,
        McpConfig,
        Origin,
        ServerConfig,
        Settings,
        Sources,
        StdioConfig,
        Transport,
    },
    connection::Annotations,
    ui::{
        self,
        Act,
        Defined,
        Host,
        McpUi,
        PendingRow,
        ServerRow,
        Servers,
        ToolRow,
        card,
        page::{self, Ui},
    },
};
use tau_store::Store;
use tau_ui_plugin::{
    CallData,
    CallResult,
    ConfigDir,
    Handle,
    HostCx,
    RepoCtx,
    Request,
    RunInfo,
    SavedSettings,
    Services,
    UiPlugin as _,
    ViewCx,
    points::{AtCard, AtRepo, AtRun},
};

// The settings' laws.

/// A small pool of names, two of which share a namespace, and one the
/// user's file has.
const NAMES: [&str; 5] = ["git", "linear", "a-b", "a_b", "mine"];
const USER: &str = "mine";

/// What the page can ask about the settings' servers.
#[derive(Debug, Clone)]
enum Step {
    Add(usize, String),
    Edit(usize, String),
    Remove(usize),
    Enable(usize, bool),
}

hegel::pretty_print_as_debug!(Step);

#[hegel::composite]
fn step(tc: &TestCase) -> Step {
    let name: usize = tc.draw(gs::integers().max_value(NAMES.len() - 1));
    let command: String = tc.draw(gs::from_regex("[a-z]{1,6}"));
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => Step::Add(name, command),
        1 => Step::Edit(name, command),
        2 => Step::Remove(name),
        _ => Step::Enable(name, tc.draw(gs::booleans())),
    }
}

fn namespace(name: &str) -> String {
    name.replace('-', "_")
}

/// Whatever the page asks, in whatever order, the settings' servers are
/// a model's: a server is added only under a new name that neither the
/// user's file nor another server's namespace takes, and edited,
/// removed or turned on and off only when the page added it. Every
/// entry the settings keep parses, and the settings survive saving.
#[hegel::test(test_cases = 200)]
fn settings_follow_the_actions(tc: TestCase) {
    let steps: Vec<Step> = tc.draw(gs::vecs(step()).max_size(16));
    let user: BTreeSet<String> = [USER.to_owned()].into();
    let mut settings = Settings::default();
    // Name to (command, enabled).
    let mut model: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for step in steps {
        let (act, expected) = match step {
            Step::Add(n, command) => {
                let name = NAMES[n].to_owned();
                let free = !user.contains(&name)
                    && !model.contains_key(&name)
                    && !model
                        .keys()
                        .chain(user.iter())
                        .any(|other| namespace(other) == namespace(&name));
                let mut next = model.clone();
                if free {
                    next.insert(name.clone(), (command.clone(), true));
                }
                (
                    Act::Add {
                        name,
                        entry: json!({ "command": command }),
                    },
                    free.then_some(next),
                )
            }
            Step::Edit(n, command) => {
                let name = NAMES[n].to_owned();
                let mut next = model.clone();
                let ok = next
                    .get_mut(&name)
                    .map(|server| *server = (command.clone(), true))
                    .is_some();
                (
                    Act::Edit {
                        name,
                        entry: json!({ "command": command }),
                    },
                    ok.then_some(next),
                )
            }
            Step::Remove(n) => {
                let name = NAMES[n].to_owned();
                let mut next = model.clone();
                let ok = next.remove(&name).is_some();
                (Act::Remove { name }, ok.then_some(next))
            }
            Step::Enable(n, enabled) => {
                let name = NAMES[n].to_owned();
                let mut next = model.clone();
                let ok = next
                    .get_mut(&name)
                    .map(|server| server.1 = enabled)
                    .is_some();
                (Act::Enable { name, enabled }, ok.then_some(next))
            }
        };
        let result = ui::apply(&settings, &user, &[], &act);
        match expected {
            Some(next) => {
                settings = result
                    .unwrap_or_else(|error| panic!("{act:?} refused: {error}"));
                model = next;
            }
            None => assert!(result.is_err(), "{act:?} was let through"),
        }
        let (config, errors) = settings.config();
        assert!(errors.is_empty(), "{errors:?}");
        let kept: BTreeMap<String, (String, bool)> = config
            .servers
            .iter()
            .map(|server| {
                let Transport::Stdio(stdio) = &server.transport else {
                    panic!("only commands are added");
                };
                (server.name.clone(), (stdio.command.clone(), server.enabled))
            })
            .collect();
        assert_eq!(kept, model);
        let saved = serde_json::to_value(&settings).unwrap();
        assert_eq!(
            serde_json::from_value::<Settings>(saved).unwrap(),
            settings
        );
    }
    assert!(settings.approved.is_empty(), "nothing was approved");
}

#[hegel::composite]
fn repo_server(tc: &TestCase) -> ServerConfig {
    let name: String = tc.draw(gs::from_regex("[a-z][a-z0-9_-]{0,8}"));
    let transport = if tc.draw(gs::booleans()) {
        Transport::Stdio(StdioConfig {
            command: tc.draw(gs::from_regex("[a-z/._-]{0,10}[a-z]")),
            args: tc.draw(gs::vecs(gs::text().max_size(8)).max_size(3)),
            env: Vec::new(),
            cwd: None,
        })
    } else {
        Transport::Http(HttpConfig {
            url: tc.draw(gs::from_regex("https://[a-z]{1,10}\\.dev/mcp")),
            headers: Vec::new(),
        })
    };
    let mut server = ServerConfig::new(name, transport);
    server.exposure = Exposure::ALL[tc.draw(gs::integers().max_value(2_usize))];
    server.enabled = tc.draw(gs::booleans());
    server.description = tc.draw(gs::optional(gs::text().max_size(12)));
    server
}

/// A repository's server waits for approval under the hash of its
/// entry, which reading the file again, however it is laid out, does
/// not change. Approving that hash connects it, as the repository's;
/// approving any other refuses and saves nothing; and a change to the
/// entry asks again.
#[hegel::test(test_cases = 200)]
fn an_approval_holds_for_the_entry_shown(tc: TestCase) {
    let server = tc.draw(repo_server().print_as_debug());
    let wrong: String = tc.draw(gs::from_regex("[0-9a-f]{64}"));
    let file = McpConfig {
        servers: vec![server],
    }
    .to_value();
    // As the file is read, compact or laid out: the same entry.
    let (read, errors) = McpConfig::parse(&file.to_string());
    assert!(errors.is_empty(), "{errors:?}");
    let server = read.servers[0].clone();
    let (reread, _) =
        McpConfig::parse(&serde_json::to_string_pretty(&file).unwrap());
    assert_eq!(reread.servers[0].approval_hash(), server.approval_hash());

    let settings = Settings::default();
    let sources = Sources::merge(None, &settings, Some(&reread));
    assert_eq!(sources.pending.len(), 1);
    assert_eq!(sources.pending[0].hash, server.approval_hash());
    assert!(sources.servers.is_empty());
    let approve = |hash: &str| Act::Approve {
        repo: "r".into(),
        server: server.name.clone(),
        hash: hash.to_owned(),
    };
    let none = BTreeSet::new();
    if wrong != server.approval_hash() {
        assert!(
            ui::apply(&settings, &none, &sources.pending, &approve(&wrong))
                .is_err()
        );
    }
    let approved = ui::apply(
        &settings,
        &none,
        &sources.pending,
        &approve(&server.approval_hash()),
    )
    .unwrap();
    assert_eq!(approved.approved, BTreeSet::from([server.approval_hash()]));
    let sources = Sources::merge(None, &approved, Some(&reread));
    assert!(sources.pending.is_empty());
    assert_eq!(sources.servers, [(Origin::Repo, server.clone())]);
    // A commit changes the entry: it waits again.
    let mut changed = server.clone();
    changed.timeout += 1.0;
    let file = McpConfig {
        servers: vec![changed],
    };
    let sources = Sources::merge(None, &approved, Some(&file));
    assert_eq!(sources.pending.len(), 1);
}

/// A server the page cannot add says why: a bad name, an entry that is
/// not a server.
#[test]
fn bad_entries_are_refused() {
    let none = BTreeSet::new();
    let add = |name: &str, entry: Value| {
        ui::apply(
            &Settings::default(),
            &none,
            &[],
            &Act::Add {
                name: name.into(),
                entry,
            },
        )
    };
    assert!(add("has space", json!({ "command": "x" })).is_err());
    assert!(add("x", json!({ "type": "sse", "url": "https://a" })).is_err());
    assert!(add("x", json!({})).is_err());
    let added = add("x", json!({ "command": "x", "type": "stdio" })).unwrap();
    // As tau prints it: the default type left out.
    assert_eq!(added.servers["x"], json!({ "command": "x" }));
}

// The host.

/// A host over a temporary user directory and one repository, with its
/// settings in memory.
struct Fixture {
    _dirs: (tempfile::TempDir, tempfile::TempDir),
    runtime: tokio::runtime::Runtime,
    host: Option<Host>,
    cx: HostCx,
    repo: RepoCtx,
}

impl Fixture {
    fn new(user: Option<Value>, repo: Option<Value>) -> Self {
        let user_dir = tempfile::tempdir().unwrap();
        let repo_dir = tempfile::tempdir().unwrap();
        if let Some(user) = user {
            std::fs::write(user_dir.path().join("mcp.json"), user.to_string())
                .unwrap();
        }
        if let Some(repo) = repo {
            std::fs::create_dir_all(repo_dir.path().join(".tau")).unwrap();
            std::fs::write(
                repo_dir.path().join(".tau/mcp.json"),
                repo.to_string(),
            )
            .unwrap();
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(Store::memory()).unwrap();
        let repo = RepoCtx {
            name: "r".into(),
            checkout: repo_dir.path().to_owned(),
            dir: repo_dir.path().join("tau"),
        };
        let cx = HostCx::new(
            store,
            runtime.handle().clone(),
            Services::default()
                .with(SavedSettings::in_memory())
                .with(ConfigDir(user_dir.path().to_owned())),
            user_dir.path().join("data"),
            vec![repo.clone()],
            Arc::new(|_| {}),
        );
        let host = McpUi.host(&cx).unwrap();
        Self {
            _dirs: (user_dir, repo_dir),
            runtime,
            host: Some(host),
            cx,
            repo,
        }
    }

    fn host(&self) -> &Host {
        self.host.as_ref().unwrap()
    }

    fn act(&self, act: Act) -> anyhow::Result<()> {
        McpUi
            .act(self.host(), serde_json::to_value(act).unwrap(), &self.cx)
            .map(drop)
    }

    fn settings(&self) -> Settings {
        self.cx.settings(NAME)
    }

    fn repo_data(&self) -> Servers {
        McpUi.repo_data(self.host(), &self.repo, &self.cx)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Closes the connections on the runtime before it goes.
        let host = self.host.take();
        self.runtime.block_on(async move { drop(host) });
    }
}

/// A command that is not there: its server fails to start, and nothing
/// runs.
const MISSING: &str = "/nonexistent/tau-mcp-test-server";

/// Approving a repository's server through the page saves the hash of
/// the entry it showed; from then on the server is the repository's,
/// not pending.
#[test]
fn approving_saves_the_hash_shown() {
    let fixture = Fixture::new(
        None,
        Some(
            json!({ "mcpServers": { "git": { "command": MISSING, "args": ["x"] } } }),
        ),
    );
    let data = fixture.repo_data();
    assert_eq!(data.pending.len(), 1);
    let pending = data.pending[0].clone();
    assert_eq!(pending.entry, json!({ "command": MISSING, "args": ["x"] }));
    let (file, _) = McpConfig::from_value(
        &json!({ "mcpServers": { "git": { "command": MISSING, "args": ["x"] } } }),
    );
    assert_eq!(pending.hash, file.servers[0].approval_hash());
    // Another hash is not the entry shown.
    assert!(
        fixture
            .act(Act::Approve {
                repo: "r".into(),
                server: "git".into(),
                hash: "0".repeat(64),
            })
            .is_err()
    );
    assert!(fixture.settings().approved.is_empty());
    fixture
        .act(Act::Approve {
            repo: "r".into(),
            server: "git".into(),
            hash: pending.hash.clone(),
        })
        .unwrap();
    assert_eq!(fixture.settings().approved, BTreeSet::from([pending.hash]));
    let data = fixture.repo_data();
    assert!(data.pending.is_empty());
    assert_eq!(data.servers.len(), 1);
    assert_eq!(data.servers[0].defined, Defined::Repo);
    assert!(!data.started, "approving starts nothing");
}

/// The page adds, edits, turns off and removes its own servers, and
/// refuses a name the user's file has.
#[test]
fn the_page_edits_only_its_own_servers() {
    let fixture = Fixture::new(
        Some(
            json!({ "mcpServers": { "linear": { "url": "https://linear.app/mcp" } } }),
        ),
        None,
    );
    let refused = fixture.act(Act::Add {
        name: "linear".into(),
        entry: json!({ "command": "x" }),
    });
    assert!(refused.unwrap_err().to_string().contains("mcp.json"));
    assert!(fixture.settings().servers.is_empty());
    // Nor can it edit or remove the user's.
    assert!(
        fixture
            .act(Act::Remove {
                name: "linear".into()
            })
            .is_err()
    );

    fixture
        .act(Act::Add {
            name: "git".into(),
            entry: json!({ "command": MISSING }),
        })
        .unwrap();
    fixture
        .act(Act::Enable {
            name: "git".into(),
            enabled: false,
        })
        .unwrap();
    assert_eq!(
        fixture.settings().servers["git"],
        json!({ "command": MISSING, "enabled": false })
    );
    let data = McpUi.data(fixture.host(), &fixture.cx);
    let names: Vec<(&str, Defined, bool)> = data
        .servers
        .iter()
        .map(|s| (s.name.as_str(), s.defined, s.enabled))
        .collect();
    assert_eq!(
        names,
        [
            ("linear", Defined::User, true),
            ("git", Defined::Settings, false)
        ]
    );
    assert_eq!(data.user_names, BTreeSet::from(["linear".to_owned()]));
    // Only the page's own server carries its entry, for the editor.
    assert!(data.servers[0].entry.is_none());
    assert!(data.servers[1].entry.is_some());

    fixture
        .act(Act::Edit {
            name: "git".into(),
            entry: json!({ "command": MISSING, "args": ["serve"] }),
        })
        .unwrap();
    assert_eq!(
        fixture.settings().servers["git"],
        json!({ "command": MISSING, "args": ["serve"] })
    );
    fixture.act(Act::Remove { name: "git".into() }).unwrap();
    assert!(fixture.settings().servers.is_empty());
}

/// The host keeps one plugin per repository, built when a run or the
/// page first needs it, shared until its servers change, and built
/// again when they do.
#[test]
fn a_repository_keeps_its_plugin_until_its_servers_change() {
    let fixture = Fixture::new(
        Some(json!({ "mcpServers": { "a": { "command": MISSING } } })),
        None,
    );
    let checkout = fixture.repo.checkout.clone();
    assert!(!fixture.repo_data().started);
    let settings = fixture.settings();
    let first = fixture.host().plugin(Some(&checkout), &settings);
    let again = fixture.host().plugin(Some(&checkout), &settings);
    assert!(Arc::ptr_eq(
        &first.connections()[0],
        &again.connections()[0]
    ));
    assert!(fixture.repo_data().started);
    // The user's servers alone are another scope.
    let user = fixture.host().plugin(None, &settings);
    assert!(!Arc::ptr_eq(
        &first.connections()[0],
        &user.connections()[0]
    ));
    // A server added on the page: the repository's plugin is built again.
    fixture
        .act(Act::Add {
            name: "b".into(),
            entry: json!({ "command": MISSING }),
        })
        .unwrap();
    let settings = fixture.settings();
    let rebuilt = fixture.host().plugin(Some(&checkout), &settings);
    assert_eq!(rebuilt.connections().len(), 2);
    assert!(!Arc::ptr_eq(
        &first.connections()[0],
        &rebuilt.connections()[0]
    ));
    // So is one the user writes in the file by hand.
    std::fs::write(
        fixture._dirs.0.path().join("mcp.json"),
        json!({ "mcpServers": { "a": { "command": MISSING, "args": ["v2"] } } })
            .to_string(),
    )
    .unwrap();
    let edited = fixture.host().plugin(Some(&checkout), &settings);
    assert!(!Arc::ptr_eq(
        &rebuilt.connections()[0],
        &edited.connections()[0]
    ));
    // A repository with no server that would connect adds nothing to runs.
    let empty = Fixture::new(None, None);
    let run = tau_ui_plugin::RunCtx {
        kind: tau_ui_plugin::RunKind::Main,
        repo: empty.repo.clone(),
        model: "m".into(),
        effort: None,
        services: Services::default(),
    };
    let plugins = McpUi
        .agent_plugins(empty.host(), &run, &Settings::default())
        .unwrap();
    assert!(plugins.is_empty());
    let run = tau_ui_plugin::RunCtx {
        repo: fixture.repo.clone(),
        ..run
    };
    let plugins = McpUi
        .agent_plugins(fixture.host(), &run, &settings)
        .unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].name(), NAME);
}

/// Connect on the page starts the servers of a repository no run has
/// started.
#[test]
fn connect_starts_a_repository_servers() {
    let fixture = Fixture::new(
        Some(json!({ "mcpServers": { "a": { "command": MISSING } } })),
        None,
    );
    assert!(!fixture.repo_data().started);
    fixture
        .act(Act::Reconnect {
            repo: Some("r".into()),
            server: None,
        })
        .unwrap();
    let data = fixture.repo_data();
    assert!(data.started);
    assert!(data.servers[0].state.is_some());
    assert!(
        fixture
            .act(Act::Reconnect {
                repo: Some("nowhere".into()),
                server: None,
            })
            .is_err()
    );
}

/// The catalog says how many servers there are, how many are connected
/// and how many wait.
#[test]
fn summaries_count_what_matters() {
    assert_eq!(ui::summary(0, None, 0), "no servers");
    assert_eq!(ui::summary(1, None, 0), "1 server");
    assert_eq!(
        ui::summary(3, Some(2), 1),
        "3 servers · 2 connected · 1 needs approval"
    );
    assert_eq!(ui::summary(0, Some(0), 2), "no servers · 2 need approval");
}

// The interface.

type Asked = Rc<RefCell<Vec<Request>>>;

fn window_ui(cx: &mut TestAppContext) -> (Entity<Ui>, Asked) {
    let asked: Asked = Rc::default();
    let sink = asked.clone();
    let handle = Handle::new(
        NAME,
        Rc::new(move |_, request, _: &mut App| sink.borrow_mut().push(request)),
    );
    let ui = cx.update(|cx| cx.new(|cx| McpUi.new_ui(handle, cx)));
    (ui, asked)
}

fn acts(asked: &Asked) -> Vec<Act> {
    asked
        .borrow()
        .iter()
        .filter_map(|request| match request {
            Request::Act(action) => serde_json::from_value(action.clone()).ok(),
            _ => None,
        })
        .collect()
}

/// The editor sends only a server it may add, and says why otherwise;
/// removing asks twice; the rest is sent as clicked.
#[gpui::test]
fn the_editor_sends_what_it_may(cx: &mut TestAppContext) {
    let (ui, asked) = window_ui(cx);
    let user = BTreeSet::from(["linear".to_owned()]);
    ui.update(cx, |ui, cx| {
        ui.open_add(cx);
        ui.set_name("linear", cx);
        ui.set_entry("{ \"command\": \"x\" }", cx);
        assert!(!ui.save(&user, cx));
        assert!(
            ui.editor()
                .unwrap()
                .problem
                .as_deref()
                .unwrap()
                .contains("mcp.json")
        );
        ui.set_name("git", cx);
        ui.set_entry("{ \"command\": ", cx);
        assert!(!ui.save(&user, cx));
        assert!(
            ui.editor()
                .unwrap()
                .problem
                .as_deref()
                .unwrap()
                .starts_with("The entry is not JSON")
        );
        ui.set_entry(
            "{ \"command\": \"uvx\", \"args\": [\"mcp-server-git\"] }",
            cx,
        );
        assert!(ui.save(&user, cx));
        assert!(ui.editor().is_none());
        ui.open_edit("git", &json!({ "command": "uvx" }), cx);
        ui.set_entry("{ \"command\": \"git-mcp\" }", cx);
        assert!(ui.save(&user, cx));
        ui.remove("git", cx);
        assert_eq!(ui.removing(), Some("git"));
        ui.remove("git", cx);
        ui.enable("git", false, cx);
        ui.approve(
            "r",
            &PendingRow {
                name: "db".into(),
                hash: "h".into(),
                ..PendingRow::default()
            },
            cx,
        );
        ui.reconnect(Some("r"), Some("db"), cx);
    });
    assert_eq!(
        acts(&asked),
        [
            Act::Add {
                name: "git".into(),
                entry: json!({ "command": "uvx", "args": ["mcp-server-git"] }),
            },
            Act::Edit {
                name: "git".into(),
                entry: json!({ "command": "git-mcp" }),
            },
            Act::Remove { name: "git".into() },
            Act::Enable {
                name: "git".into(),
                enabled: false,
            },
            Act::Approve {
                repo: "r".into(),
                server: "db".into(),
                hash: "h".into(),
            },
            Act::Reconnect {
                repo: Some("r".into()),
                server: Some("db".into()),
            },
        ]
    );
}

fn info(repo: &str) -> RunInfo {
    RunInfo {
        id: RunId("run".into()),
        repo: repo.into(),
        live: true,
        title: "run".into(),
        answer: None,
        context: 0,
        window: None,
    }
}

/// A repository with a connected server and its tools, one that failed,
/// and one waiting for approval.
fn servers() -> Servers {
    Servers {
        started: true,
        servers: vec![
            ServerRow {
                name: "git".into(),
                defined: Defined::Settings,
                transport: "uvx mcp-server-git".into(),
                exposure: "direct".into(),
                enabled: true,
                state: Some("connected".into()),
                tools: vec![
                    ToolRow {
                        name: Some("mcp__git__status".into()),
                        tool: "status".into(),
                        description: Some("Shows the status.".into()),
                        exposure: "direct".into(),
                        annotations: Annotations {
                            read_only: Some(true),
                            ..Annotations::default()
                        },
                    },
                    ToolRow {
                        name: Some("mcp__git__push".into()),
                        tool: "push".into(),
                        description: None,
                        exposure: "codemode".into(),
                        annotations: Annotations {
                            destructive: Some(true),
                            open_world: Some(true),
                            ..Annotations::default()
                        },
                    },
                ],
                entry: Some(
                    json!({ "command": "uvx", "args": ["mcp-server-git"] }),
                ),
                ..ServerRow::default()
            },
            ServerRow {
                name: "linear".into(),
                defined: Defined::User,
                transport: "https://mcp.linear.app/mcp (headers Authorization)"
                    .into(),
                exposure: "direct".into(),
                enabled: true,
                state: Some("failed".into()),
                error: Some("HTTP 401".into()),
                ..ServerRow::default()
            },
        ],
        pending: vec![PendingRow {
            name: "db".into(),
            transport: "./scripts/db-mcp".into(),
            entry: json!({ "command": "./scripts/db-mcp" }),
            hash: "abc".into(),
        }],
        errors: vec![
            "repository file: server `x`: `command` must be a string".into(),
        ],
        user_names: BTreeSet::from(["linear".to_owned()]),
        user_file: Some("~/.config/tau/mcp.json".into()),
        repo_file: Some("/src/r/.tau/mcp.json".into()),
    }
}

/// Runs `f` with the page's view of `repos`, at `params`.
fn with_view<R>(
    cx: &mut TestAppContext,
    ui: &Entity<Ui>,
    repos: &BTreeMap<String, Servers>,
    params: &BTreeMap<String, String>,
    run: Option<&RunInfo>,
    compact: bool,
    f: impl FnOnce(&mut ViewCx<'_, McpUi>) -> R,
) -> R {
    let list = Vec::new;
    let cards = |_: &RunId| Vec::new();
    let handle = Handle::new(NAME, Rc::new(|_, _, _: &mut App| {}));
    let data = Servers::default();
    let settings = Settings::default();
    cx.update(|cx| {
        cx.set_global(tau_ui_kit::theme::Theme::graphite());
        let mut view = ViewCx::new(
            &McpUi,
            ui.clone(),
            None,
            &data,
            &settings,
            repos,
            run,
            params,
            compact,
            false,
            1400.,
            handle,
            &list,
            &cards,
            cx,
        );
        f(&mut view)
    })
}

/// The page, drawn in a window as tau-ui draws it.
struct PageView {
    ui: Entity<Ui>,
    repos: BTreeMap<String, Servers>,
    params: BTreeMap<String, String>,
    compact: bool,
}

impl Render for PageView {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let list = Vec::new;
        let cards = |_: &RunId| Vec::new();
        let handle = Handle::new(NAME, Rc::new(|_, _, _: &mut App| {}));
        let (data, settings) = (Servers::default(), Settings::default());
        let (repos, params) = (self.repos.clone(), self.params.clone());
        let mut view = ViewCx::new(
            &McpUi,
            self.ui.clone(),
            None,
            &data,
            &settings,
            &repos,
            None,
            &params,
            self.compact,
            false,
            1400.,
            handle,
            &list,
            &cards,
            cx,
        );
        div().size_full().flex().child(page::render(&mut view))
    }
}

/// Draws the page at `params` in a window, on a computer or a phone.
fn draw_page(
    cx: &mut TestAppContext,
    ui: &Entity<Ui>,
    repos: &BTreeMap<String, Servers>,
    params: BTreeMap<String, String>,
    compact: bool,
) {
    cx.update(|cx| cx.set_global(tau_ui_kit::theme::Theme::graphite()));
    let window = cx.add_window({
        let (ui, repos) = (ui.clone(), repos.clone());
        move |_, _| PageView {
            ui,
            repos,
            params,
            compact,
        }
    });
    let cx = VisualTestContext::from_window(*window, cx);
    let width = if compact { 400. } else { 1400. };
    cx.simulate_resize(size(px(width), px(900.)));
    cx.run_until_parked();
}

/// The page draws a repository's servers on a computer and a phone, the
/// user's alone without one; the sidebar counts the servers and what
/// waits; the run's line sums them up.
#[gpui::test]
fn the_page_and_the_sidebar_draw_the_servers(cx: &mut TestAppContext) {
    let (ui, _) = window_ui(cx);
    let repos = BTreeMap::from([("r".to_owned(), servers())]);
    let at_repo = BTreeMap::from([("repo".to_owned(), "r".to_owned())]);
    with_view(cx, &ui, &repos, &at_repo, None, false, |view| {
        let (repo, shown) = page::servers_of(view);
        assert_eq!(repo.as_deref(), Some("r"));
        assert_eq!(shown.servers.len(), 2);
    });
    for compact in [false, true] {
        draw_page(cx, &ui, &repos, at_repo.clone(), compact);
    }
    // Without a repository, the user's servers alone; the editor, open,
    // draws over the page.
    with_view(cx, &ui, &repos, &BTreeMap::new(), None, false, |view| {
        let (repo, shown) = page::servers_of(view);
        assert_eq!((repo, shown.servers.len()), (None, 0));
    });
    ui.update(cx, |ui, cx| ui.open_add(cx));
    draw_page(cx, &ui, &repos, BTreeMap::new(), false);
    let entry =
        with_view(cx, &ui, &repos, &BTreeMap::new(), None, false, |view| {
            ui::sidebar(&AtRepo { repo: "r".into() }, view)
        })
        .unwrap();
    assert_eq!(entry.label, "MCP");
    assert_eq!(entry.detail.as_deref(), Some("2 servers"));
    assert_eq!(entry.badge.as_ref().map(|(n, _)| n.as_str()), Some("1"));
    assert_eq!(entry.to.params["repo"], "r");
    let run = info("r");
    let status = with_view(
        cx,
        &ui,
        &repos,
        &BTreeMap::new(),
        Some(&run),
        false,
        |view| ui::status(&AtRun { run: run.clone() }, view),
    )
    .unwrap();
    assert_eq!(status.state, "2 servers · 1 connected · 1 needs approval");
    // A repository without servers says nothing in the run's list.
    let quiet = info("elsewhere");
    let repos = BTreeMap::from([("elsewhere".to_owned(), Servers::default())]);
    assert!(
        with_view(
            cx,
            &ui,
            &repos,
            &BTreeMap::new(),
            Some(&quiet),
            false,
            |view| { ui::status(&AtRun { run: quiet.clone() }, view) }
        )
        .is_none()
    );
}

fn at_card(tool: &str, data: CallData) -> AtCard {
    AtCard {
        run: info("r"),
        call_id: "call_1".into(),
        tool: tool.into(),
        keys: Vec::new(),
        data: Arc::new(data),
        summary: String::new(),
        cut: None,
    }
}

/// A call's card names its server and tool and their hints while it
/// runs, from the repository's servers, and once it ended, from its
/// details; it shows the structured result when there is one, else the
/// text, and goes red when the call failed.
#[gpui::test]
fn the_card_shows_the_server_and_the_result(cx: &mut TestAppContext) {
    let (ui, _) = window_ui(cx);
    let repos = BTreeMap::from([("r".to_owned(), servers())]);
    let draw = |cx: &mut TestAppContext, at: AtCard| {
        with_view(cx, &ui, &repos, &BTreeMap::new(), None, false, |view| {
            card::card(&at, view)
                .map(|card| (card.failed, card.folds, card.edge.is_some()))
        })
    };
    assert!(draw(cx, at_card("bash", CallData::default())).is_none());
    let running = CallData {
        args: json!({ "path": "." }),
        ..CallData::default()
    };
    assert_eq!(
        draw(cx, at_card("mcp__git__push", running.clone())),
        Some((None, false, false))
    );
    let listed = card::shown(
        &running,
        Some((
            "git".into(),
            "push".into(),
            servers().servers[0].tools[1].annotations,
        )),
    );
    assert_eq!(
        page::hints(&listed.annotations),
        ["destructive", "open world"]
    );

    let mut done = running.clone();
    done.result = Some(CallResult {
        text: "{\"clean\":true}".into(),
        details: Some(json!({
            "server": "git", "tool": "status",
            "annotations": { "readOnlyHint": true },
            "structuredContent": { "clean": true },
        })),
        error: false,
    });
    let shown = card::shown(&done, None);
    assert_eq!(
        (shown.server.as_deref(), shown.tool.as_deref()),
        (Some("git"), Some("status"))
    );
    assert_eq!(page::hints(&shown.annotations), ["read-only"]);
    assert_eq!(shown.structured.as_deref(), Some("{\n  \"clean\": true\n}"));
    assert_eq!(shown.text, None);
    assert_eq!(
        draw(cx, at_card("mcp__git__status", done)),
        Some((None, true, false))
    );

    let mut failed = running;
    failed.result = Some(CallResult {
        text: "MCP tool git/push returned an error\nmore".into(),
        details: Some(
            json!({ "server": "git", "tool": "push", "annotations": {} }),
        ),
        error: true,
    });
    let shown = card::shown(&failed, None);
    assert_eq!(
        shown.text.as_deref(),
        Some("MCP tool git/push returned an error\nmore")
    );
    assert_eq!(
        draw(cx, at_card("mcp__git__push", failed)),
        Some((
            Some("MCP tool git/push returned an error".into()),
            true,
            true
        ))
    );
}

/// The UI takes its look from the kit.
#[test]
fn only_the_kit_holds_design_values() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
