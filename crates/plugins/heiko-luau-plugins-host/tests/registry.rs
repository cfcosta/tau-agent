//! The plugins repository's registry (ADR 0027): a version activates by
//! itself when its tests pass and it reaches nothing it was not allowed;
//! one that reaches further waits for the person, and one whose tests
//! fail or that does not load leaves the version before active.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use heiko_luau_plugins::Standing;
use heiko_luau_plugins_host::{
    registry::Registry,
    runtime::{Files, Loaded, Manifest, load},
};
use heiko_testing::block_on_io;
use heiko_vcs_host::{Identity, Project, ProjectRepo};

const TOOL_ONLY: &str = r#"
local heiko = require("heiko")
return heiko.plugin {
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
}
"#;

const BLOCKING: &str = r#"
local heiko = require("heiko")
return heiko.plugin {
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  before_tool = function(call, model, env)
    if call.name == "exec_command" then return heiko.block("no") end
  end,
}
"#;

const PASSING: &str = r#"
local heiko = require("heiko")
local t = heiko.test
t.case("says hi", function() t.equal(t.run{}:tool("hello"), "hi") end)
"#;

const FAILING: &str = r#"
local heiko = require("heiko")
local t = heiko.test
t.case("says bye", function() t.equal(t.run{}:tool("hello"), "bye") end)
"#;

/// What `greet`'s `heiko.toml` says of `plugin`.
fn described(plugin: &str) -> &'static str {
    if plugin == BLOCKING {
        "Says hello, and blocks rm."
    } else {
        "Says hello."
    }
}

/// A project whose trunk holds the plugin `greet` with `plugin` and,
/// when given, a test file.
fn project(
    home: &std::path::Path,
    n: usize,
    plugin: &str,
    test: Option<&str>,
) -> (Project, String) {
    let mut files = vec![
        (
            "greet/heiko.toml".to_owned(),
            Manifest::text("greet", described(plugin)),
        ),
        ("greet/src/plugin.luau".to_owned(), plugin.to_owned()),
    ];
    if let Some(test) = test {
        files.push(("greet/tests/basic.luau".to_owned(), test.to_owned()));
    }
    let repo = ProjectRepo::init(
        home.join(format!("v{n}")),
        Identity::default(),
        &files,
    )
    .unwrap();
    let trunk = repo.trunk().unwrap();
    (Project::from(repo), trunk)
}

fn standing(registry: &Registry) -> Standing {
    block_on_io(registry.overview()).plugins[0].standing.clone()
}

/// What the page shows `greet`'s waiting version asking for.
fn shown(registry: &Registry) -> Vec<String> {
    match standing(registry) {
        Standing::Waiting { grown } => grown,
        _ => Vec::new(),
    }
}

/// The person allows `greet` from its page, as drawn now.
fn allow(registry: &Registry) -> anyhow::Result<()> {
    block_on_io(registry.allow("greet", &shown(registry)))
}

fn active_description(registry: &Registry) -> Option<String> {
    block_on_io(registry.active())
        .first()
        .map(|active| active.loaded.declaration.description.clone())
}

#[test]
fn versions_activate_wait_or_keep_the_one_before() {
    let home = tempfile::tempdir().unwrap();
    let refreshed = Arc::new(AtomicUsize::new(0));
    let seen = refreshed.clone();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        move || {
            seen.fetch_add(1, Ordering::SeqCst);
        },
    );

    // A tool, reaching nothing: it activates by itself.
    let (v1, trunk) = project(home.path(), 1, TOOL_ONLY, Some(PASSING));
    block_on_io(registry.reload(&v1, &trunk));
    assert_eq!(standing(&registry), Standing::Active);
    assert_eq!(
        active_description(&registry).as_deref(),
        Some("Says hello.")
    );
    let overview = block_on_io(registry.overview());
    assert_eq!(overview.commit.as_deref(), Some(trunk.as_str()));
    assert!(overview.plugins[0].tests.iter().all(|test| test.passed));
    assert_eq!(refreshed.load(Ordering::SeqCst), 1);

    // It gains `before_tool`: it waits, and the version before stays.
    let (v2, trunk) = project(home.path(), 2, BLOCKING, Some(PASSING));
    block_on_io(registry.reload(&v2, &trunk));
    match standing(&registry) {
        Standing::Waiting { grown } => assert_eq!(grown, ["`before_tool`"]),
        other => panic!("{other:?}"),
    }
    assert!(block_on_io(registry.overview()).plugins[0].keeps_earlier);
    assert_eq!(
        active_description(&registry).as_deref(),
        Some("Says hello.")
    );

    // Allowed from a page drawn before it asked for more: not allowed.
    assert!(block_on_io(registry.allow("greet", &[])).is_err());
    assert!(matches!(standing(&registry), Standing::Waiting { .. }));

    // Allowed, it activates; read again, it stays active.
    allow(&registry).unwrap();
    assert_eq!(standing(&registry), Standing::Active);
    assert_eq!(
        active_description(&registry).as_deref(),
        Some("Says hello, and blocks rm.")
    );
    block_on_io(registry.reload(&v2, &trunk));
    assert_eq!(standing(&registry), Standing::Active);
    assert!(allow(&registry).is_err(), "nothing waits");

    // Its tests fail: the version before stays, and it cannot be
    // allowed, as from a page that still showed it waiting.
    let (v3, trunk) = project(home.path(), 3, BLOCKING, Some(FAILING));
    block_on_io(registry.reload(&v3, &trunk));
    assert_eq!(standing(&registry), Standing::Failing);
    assert!(
        allow(&registry).is_err(),
        "a version whose tests fail is never allowed"
    );
    assert_eq!(standing(&registry), Standing::Failing);
    assert_eq!(
        active_description(&registry).as_deref(),
        Some("Says hello, and blocks rm.")
    );

    // It does not load: the version before stays, and the error shows.
    let (v4, trunk) = project(home.path(), 4, "return heiko.plugin {", None);
    block_on_io(registry.reload(&v4, &trunk));
    assert!(matches!(standing(&registry), Standing::Broken { .. }));
    assert_eq!(block_on_io(registry.active()).len(), 1);
}

/// What the person allowed outlives the registry: a new one reading the
/// same version activates it.
#[test]
fn what_was_allowed_is_kept() {
    let home = tempfile::tempdir().unwrap();
    let allowed = home.path().join("allowed.json");
    let (v1, trunk) = project(home.path(), 1, BLOCKING, None);
    let first = Registry::at(
        home.path().join("x"),
        allowed.clone(),
        Identity::default(),
        || {},
    );
    block_on_io(first.reload(&v1, &trunk));
    assert!(matches!(standing(&first), Standing::Waiting { .. }));
    allow(&first).unwrap();

    let second = Registry::at(
        home.path().join("x"),
        allowed,
        Identity::default(),
        || {},
    );
    block_on_io(second.reload(&v1, &trunk));
    assert_eq!(standing(&second), Standing::Active);
}

/// A plugin's own settings page is drawn on the host, of the settings it
/// is given (ADR 0029); one that is not loaded says so.
#[test]
fn a_settings_page_is_drawn_of_the_settings_given() {
    const PAGED: &str = r#"
local heiko = require("heiko")
local ui = heiko.ui
return heiko.plugin { tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  settings = {
    schema = { type = "object", properties = { loud = { type = "boolean" } } },
    default = { loud = false },
    view = function(settings, env)
      return ui.stack { ui.text(settings.loud and "loud" or "quiet"), ui.toggle("loud", "Shout") }
    end,
  },
}
"#;
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, PAGED, None);
    block_on_io(registry.reload(&v1, &trunk));
    assert_eq!(standing(&registry), Standing::Active);
    let page = block_on_io(
        registry.settings_page("greet", serde_json::json!({ "loud": true })),
    );
    assert_eq!(
        page.page.unwrap(),
        serde_json::json!({ "piece": "stack", "children": [
            { "piece": "text", "text": "loud" },
            { "piece": "toggle", "key": "loud", "label": "Shout" },
        ] })
    );
    let declared = &block_on_io(registry.overview()).plugins[0];
    assert!(declared.declaration.as_ref().unwrap().hooks.settings_view);
    assert!(
        block_on_io(registry.settings_page("other", serde_json::json!({})))
            .page
            .is_err()
    );
}

/// Writes `greet` into a workspace `dir`: its plugin and, when given, a
/// test file.
fn write_greet(dir: &std::path::Path, plugin: &str, test: Option<&str>) {
    let folder = dir.join("greet");
    std::fs::create_dir_all(folder.join("tests")).unwrap();
    std::fs::create_dir_all(folder.join("src")).unwrap();
    std::fs::write(
        folder.join("heiko.toml"),
        Manifest::text("greet", described(plugin)),
    )
    .unwrap();
    std::fs::write(folder.join("src/plugin.luau"), plugin).unwrap();
    match test {
        Some(test) => std::fs::write(folder.join("tests/basic.luau"), test),
        None => {
            std::fs::remove_file(folder.join("tests/basic.luau")).or(Ok(()))
        }
    }
    .unwrap();
}

/// A run in the plugins repository runs a plugin as its workspace has
/// it, as soon as that version passes its tests and reaches nothing it
/// was not allowed: before the plugin lands on trunk, and before trunk
/// has any plugin at all. A failing or further-reaching version leaves
/// the run as trunk has it.
#[test]
fn a_workspace_runs_its_own_passing_versions() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    // Trunk has no plugins yet.
    let registry = Registry::at(
        home.path().join("none"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let names = |registry: &Registry| -> Vec<String> {
        block_on_io(registry.in_workspace(workspace.path()))
            .iter()
            .map(|active| active.loaded.declaration.description.clone())
            .collect()
    };
    assert!(names(&registry).is_empty());

    write_greet(workspace.path(), TOOL_ONLY, Some(PASSING));
    assert_eq!(names(&registry), ["Says hello."], "a new plugin runs");

    write_greet(workspace.path(), TOOL_ONLY, Some(FAILING));
    assert!(names(&registry).is_empty(), "a failing version does not");

    // A version that reaches further than allowed waits for the person,
    // as on trunk.
    write_greet(workspace.path(), BLOCKING, None);
    assert!(names(&registry).is_empty(), "nor one that reaches further");

    // Other runs go by trunk alone.
    write_greet(workspace.path(), TOOL_ONLY, Some(PASSING));
    assert_eq!(names(&registry), ["Says hello."]);
    assert!(block_on_io(registry.active()).is_empty());
}

/// A workspace's plugins may land by themselves when what they change
/// passes and reaches nothing new (ADR 0034). A plugin the same as
/// trunk's does not count, even when trunk's fails: the workspace did
/// not break it.
#[test]
fn a_workspace_is_ready_when_what_it_changes_passes() {
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let ready = || block_on_io(registry.ready(workspace.path()));
    assert!(ready(), "no plugins, nothing to check");

    write_greet(workspace.path(), TOOL_ONLY, Some(PASSING));
    assert!(ready(), "a new plugin that passes");
    write_greet(workspace.path(), TOOL_ONLY, Some(FAILING));
    assert!(!ready(), "one whose tests fail");
    write_greet(workspace.path(), BLOCKING, Some(PASSING));
    assert!(!ready(), "one that reaches further than allowed");
    // Once the person allows that reach, for trunk's version, the same
    // workspace version, unchanged, may land.
    let (blocking, trunk) = project(home.path(), 9, BLOCKING, Some(PASSING));
    block_on_io(registry.reload(&blocking, &trunk));
    allow(&registry).unwrap();
    assert!(ready(), "it reaches only what is allowed now");
    // Trunk goes back to the version that reaches nothing.
    let (plain, trunk) = project(home.path(), 10, TOOL_ONLY, Some(PASSING));
    block_on_io(registry.reload(&plain, &trunk));

    // Trunk's failing version, unchanged in the workspace.
    let (failing, trunk) = project(home.path(), 1, TOOL_ONLY, Some(FAILING));
    block_on_io(registry.reload(&failing, &trunk));
    write_greet(workspace.path(), TOOL_ONLY, Some(FAILING));
    assert!(ready(), "trunk's own version");
}

/// A button's action runs on the state the run has, under the person's
/// settings, and its view is drawn from the state it left; one that
/// fails, or that the plugin lacks, leaves the state as it was and says
/// why (ADR 0045).
#[test]
fn an_action_changes_the_state_and_draws_it() {
    const COUNTER: &str = r#"
local heiko = require("heiko")
local ui = heiko.ui
return heiko.plugin { tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  settings = {
    schema = { type = "object", properties = { step = { type = "integer" } } },
    default = { step = 1 },
  },
  view = function(state, env)
    return { status = ui.status(tostring(state.n or 0)) }
  end,
  actions = {
    add = function(args, model, env)
      model.n = (model.n or 0) + env.settings.step * (args.times or 1)
      heiko.log("added")
    end,
    broken = function(args, model, env) error("no luck") end,
  },
}
"#;
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, COUNTER, None);
    block_on_io(registry.reload(&v1, &trunk));
    assert_eq!(standing(&registry), Standing::Active);
    let act = |action: &str, state: serde_json::Value| {
        block_on_io(registry.action(
            heiko_luau_plugins_host::registry::ActionCall {
                plugin: "greet",
                action,
                args: &serde_json::json!({ "times": 2 }),
                run: "r1",
                repo: None,
                state: &state,
                settings: Some(&serde_json::json!({ "step": 5 })),
                chats: None,
                decide: None,
                live: false,
                keyed: false,
            },
        ))
    };

    let done = act("add", serde_json::json!({ "n": 1 }));
    assert_eq!(done.error, None);
    assert_eq!(done.state, serde_json::json!({ "n": 11 }));
    assert_eq!(done.logs, ["added"]);
    assert_eq!(
        done.view.unwrap()["status"]["text"],
        serde_json::json!("11")
    );

    for action in ["broken", "missing"] {
        let failed = act(action, serde_json::json!({ "n": 1 }));
        assert_eq!(failed.state, serde_json::json!({ "n": 1 }), "{action}");
        assert_eq!(failed.view, None);
        let (hook, _) = failed.error.unwrap();
        assert_eq!(hook, format!("actions.{action}"));
    }
}

const TITLES: &str = r#"
local heiko = require("heiko")
return heiko.plugin {
  fills = { "heiko.title", "heiko.compaction" },
  title = function(run, model, env)
    return "Asked: " .. run.task
  end,
  compaction = { rewriters = { "heiko-tree-compaction" } },
}
"#;

/// A plugin that fills slots waits for the person, as any new reach does;
/// once allowed, heiko's stand-ins ask it: a run's title from its `title`,
/// cut to a title's length, and the rewriters it names beside the Luau
/// plugins' own. Before, they answer as an empty filler would.
#[test]
fn a_plugin_fills_the_title_and_compaction_slots() {
    use heiko_luau_plugins_host::slots::{LuauCompaction, LuauTitle};
    use heiko_ui_plugin::{compaction::Compaction, title::Title};
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let title = LuauTitle(registry.clone());
    let compaction = LuauCompaction(registry.clone());
    let (v1, trunk) = project(home.path(), 1, TITLES, None);
    block_on_io(registry.reload(&v1, &trunk));
    match standing(&registry) {
        Standing::Waiting { grown } => assert_eq!(
            grown,
            ["filling `heiko.title`", "filling `heiko.compaction`"]
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(block_on_io(title.write("fix it", "m")).unwrap(), None);
    assert_eq!(compaction.rewriters(), [heiko_luau_plugins::NAME]);

    allow(&registry).unwrap();
    assert_eq!(
        block_on_io(title.write("fix it", "m")).unwrap().as_deref(),
        Some("Asked: fix it")
    );
    let long = "x".repeat(100);
    let written = block_on_io(title.write(&long, "m")).unwrap().unwrap();
    assert_eq!(written.chars().count(), heiko_ui_plugin::title::MAX_CHARS);
    assert_eq!(
        compaction.rewriters(),
        [heiko_luau_plugins::NAME, "heiko-tree-compaction"]
    );
}

/// A plugin fills only the slots a Luau plugin may, and only with what
/// each needs.
#[test]
fn a_plugin_fills_only_what_it_may() {
    let refused = |source: &str| {
        let files = Files {
            source: source.into(),
            ..Files::named("greet")
        };
        block_on_io(load("greet", files)).unwrap_err()
    };
    let vcs =
        refused(r#"return require("heiko").plugin { fills = { "heiko.vcs" } }"#);
    assert!(vcs.contains("`heiko.vcs`"), "{vcs}");
    let no_hook =
        refused(r#"return require("heiko").plugin { fills = { "heiko.title" } }"#);
    assert!(no_hook.contains("a `title` hook"), "{no_hook}");
}

/// A plugin that fills `heiko.notify` sends each notice where it reaches:
/// here, a server on this computer that hears the notice's words.
#[test]
fn a_plugin_sends_heiko_s_notices_on() {
    use std::io::{Read, Write};

    use heiko_luau_plugins_host::slots::LuauNotify;
    use heiko_ui_plugin::notify::{Notice, Notify};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let heard = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = vec![0u8; 4096];
        let read = stream.read(&mut buffer).unwrap();
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        );
        String::from_utf8_lossy(&buffer[..read]).into_owned()
    });
    let source = format!(
        r#"
local heiko = require("heiko")
return heiko.plugin {{
  uses = {{ network = {{ "127.0.0.1" }} }},
  fills = {{ "heiko.notify" }},
  notify = function(notice, model, env)
    heiko.await(heiko.cmd.http.request({{ method = "POST", url = "http://127.0.0.1:{port}/", body = notice.title .. ": " .. notice.body }}))
  end,
}}
"#
    );
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    )
    .with_runtime(runtime.handle().clone());
    let (v1, trunk) = project(home.path(), 1, &source, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    LuauNotify(registry).show(
        Notice {
            title: "Ready to land".into(),
            body: "fix the login".into(),
        },
        Box::new(|| {}),
    );
    let request = heard.join().unwrap();
    assert!(
        request.ends_with("Ready to land: fix the login"),
        "{request}"
    );
}

const FORGE: &str = r#"
local heiko = require("heiko")
return heiko.plugin { uses = { secrets = { "forge-token" } },
  fills = { "heiko.forge" },
  forge = {
    name = "Gitea at home",
    id = "home",
    host = "git.home.lan",
    token = "forge-token",
    account = function(input, model, env) return "cainã" end,
    repos = function(input, model, env)
      return { { name = "me/heiko", description = "the agent", branch = "trunk" } }
    end,
    can_push = function(input, model, env) return input.repo == "me/heiko" end,
    checks = function(input, model, env) return "passed" end,
  },
}
"#;

/// A plugin that fills `heiko.forge` is the forge heiko clones from and
/// pushes to: it is named and placed by its `forge` table, signed in by
/// the secret it names, which only heiko's remote reads, and asked the
/// rest through its hooks.
#[test]
fn a_plugin_fills_the_forge_slot() {
    use heiko_luau_plugins_host::slots::LuauForge;
    use heiko_ui_plugin::forge::{Checks, Forge};
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    )
    .with_config(home.path().join("config"));
    let (v1, trunk) = project(home.path(), 1, FORGE, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    let forge = LuauForge(registry.clone());
    assert_eq!((forge.name(), forge.id()), ("Gitea at home", "home"));
    assert_eq!(forge.clone_url("me/heiko"), "https://git.home.lan/me/heiko.git");
    assert_eq!(
        forge
            .repository_of("git@git.home.lan:me/heiko.git")
            .as_deref(),
        Some("me/heiko")
    );
    assert_eq!(forge.repository_of("https://github.com/me/heiko"), None);

    // Signed out until its secret is set.
    assert_eq!(block_on_io(forge.signed_in()), None);
    let error = block_on_io(forge.remote("me/heiko")).unwrap_err().to_string();
    assert!(error.contains("forge-token"), "{error}");
    block_on_io(registry.set_secret("greet", "forge-token", "t0k3n")).unwrap();
    assert_eq!(block_on_io(forge.signed_in()).as_deref(), Some("cainã"));
    let remote = block_on_io(forge.remote("me/heiko")).unwrap();
    assert_eq!(remote.token.as_deref(), Some("t0k3n"));
    // A secret it does not name is refused.
    assert!(block_on_io(registry.set_secret("greet", "github", "x")).is_err());

    let repos = block_on_io(forge.repos()).unwrap();
    assert_eq!(repos[0].name, "me/heiko");
    assert_eq!(repos[0].branch, "trunk");
    assert!(block_on_io(forge.can_push("me/heiko")).unwrap());
    assert!(!block_on_io(forge.can_push("you/x")).unwrap());
    assert_eq!(
        block_on_io(forge.checks("me/heiko", "abc")).unwrap(),
        Checks::Passed
    );
    // A hook it lacks says so.
    let pull = heiko_ui_plugin::forge::OpenPull {
        repo: "me/heiko".into(),
        title: "t".into(),
        body: String::new(),
        head: "h".into(),
        base: "trunk".into(),
        draft: false,
        reviewers: Vec::new(),
    };
    let error = block_on_io(forge.open_pull(pull)).unwrap_err().to_string();
    assert!(error.contains("`open_pull`"), "{error}");
}

/// A repository's full name comes from every form of URL Git takes for
/// the forge's host, and from no other host's.
#[test]
fn a_forge_reads_its_own_urls() {
    use heiko_luau_plugins_host::slots::repository_of;
    for url in [
        "https://git.home.lan/me/heiko",
        "https://git.home.lan/me/heiko.git",
        "https://git.home.lan/me/heiko/",
        "ssh://git@git.home.lan/me/heiko.git",
        "git@git.home.lan:me/heiko.git",
    ] {
        assert_eq!(
            repository_of("git.home.lan", url).as_deref(),
            Some("me/heiko"),
            "{url}"
        );
    }
    for url in [
        "https://git.home.lan.evil/me/heiko",
        "https://git.home.lan/me",
        "https://git.home.lan/me/heiko/extra",
        "git@other:me/heiko.git",
    ] {
        assert_eq!(repository_of("git.home.lan", url), None, "{url}");
    }
}

const ASKS_THE_MODEL: &str = r#"
local heiko = require("heiko")
return heiko.plugin { uses = { infer = true },
  fills = { "heiko.title" },
  title = function(run, model, env)
    local ok, answer = pcall(heiko.await, heiko.cmd.infer({ task = "A title for this task", context = run.task }))
    if ok then return answer end
  end,
}
"#;

/// A hook outside any run asks the person's model with `infer`: the
/// title hook, with the run's own model, as a run's `infer` answers.
#[test]
fn a_title_hook_asks_the_persons_model() {
    use heiko_luau_plugins_host::slots::LuauTitle;
    use heiko_ui_plugin::title::Title;
    let home = tempfile::tempdir().unwrap();
    let llm = heiko_testing::scripted::ScriptedModel::new()
        .turn(|t| t.text("Fix the login"));
    let store = block_on_io(heiko_store_sqlite::memory()).unwrap();
    let reached = llm.clone();
    let model = heiko_ui_plugin::Model::new(
        move || Some(std::sync::Arc::new(reached.clone()) as _),
        store,
    );
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    )
    .with_model(model);
    let (v1, trunk) = project(home.path(), 1, ASKS_THE_MODEL, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    let title =
        block_on_io(LuauTitle(registry).write("fix the login bug", "m-run"))
            .unwrap();
    assert_eq!(title.as_deref(), Some("Fix the login"));
    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].settings.model, "m-run");
}

const COUNTS: &str = r#"
local heiko = require("heiko")
return heiko.plugin { jobs = {
    count = {
      every = 3600,
      label = "Counts",
      run = function(_, model, env) model.n = (model.n or 0) + 1 end,
    },
  },
}
"#;

/// A plugin's job is due once its time came, keeps what its model
/// left from run to run, and once run waits for its next time. A new job
/// waits for the person's Allow, as any new reach does.
#[test]
fn a_plugins_job_runs_when_due_and_keeps_its_state() {
    use heiko_luau_plugins_host::jobs;
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, COUNTS, None);
    block_on_io(registry.reload(&v1, &trunk));
    match standing(&registry) {
        Standing::Waiting { grown } => assert_eq!(grown, ["the job `count`"]),
        other => panic!("{other:?}"),
    }
    allow(&registry).unwrap();
    let dir = home.path().join("jobs");
    let ran = || std::collections::HashMap::new();
    let due = block_on_io(jobs::due_now(&registry, &dir, &mut ran()));
    assert_eq!(
        due,
        [jobs::Due {
            plugin: "greet".into(),
            job: "count".into(),
            label: "Counts".into(),
            every: 3600,
        }]
    );
    for _ in 0..2 {
        block_on_io(jobs::run(
            &registry,
            "greet",
            "count",
            &dir,
            &jobs::Ticks::default(),
            heiko_luau_plugins_host::runtime::JobRef {
                stores: registry.stores(),
                conversations: None,
            },
            Default::default(),
        ))
        .unwrap();
    }
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("greet.count.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept["state"], serde_json::json!({ "n": 2 }));
    assert!(block_on_io(jobs::due_now(&registry, &dir, &mut ran())).is_empty());
}

/// A job is due once `every` seconds passed since it last ran, and not
/// before; one that never ran is due.
#[test]
fn a_job_is_due_after_its_interval() {
    use heiko_luau_plugins_host::jobs::due;
    assert!(due(0, 60, 60));
    assert!(due(100, 60, 160));
    assert!(!due(100, 60, 159));
    assert!(!due(100, 60, 50), "a clock that went back waits");
}

/// A job that runs more often than every second keeps its plugin from
/// loading; one that ticks each second loads.
#[test]
fn a_job_runs_at_most_every_second() {
    let with = |every: u32| Files {
        source: format!(
            r#"return require("heiko").plugin {{ jobs = {{ fast = {{ every = {every}, run = function() end }} }} }}"#
        ),
        ..Files::named("greet")
    };
    let error = block_on_io(load("greet", with(0))).unwrap_err();
    assert!(error.contains("`fast`"), "{error}");
    block_on_io(load("greet", with(1))).unwrap();
}

const STEPS: &str = r#"
local heiko = require("heiko")
return heiko.plugin { uses = { store = true },
  actions = {
    begin = function(_, model, env) heiko.give(heiko.cmd.store.put("going", { n = 0 })) end,
  },
  jobs = {
    step = {
      every = 1,
      run = function(_, model, env)
        local going = heiko.await(heiko.cmd.store.get("going"))
        if going then
          going.n += 1
          heiko.give(heiko.cmd.store.put("going", going))
        end
      end,
    },
  },
}
"#;

/// A job reads and writes its plugin's store of the app, the one its
/// actions and pages use: what an action began, the job carries on, as
/// heiko-github's sign-in polls the device code its action asked for.
#[test]
fn a_job_shares_its_plugins_store_with_its_actions() {
    use heiko_luau_plugins_host::jobs;
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, STEPS, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    block_on_io(registry.app_action(
        "greet",
        "begin",
        serde_json::Value::Null,
        None,
        None,
    ))
    .unwrap();
    let dir = home.path().join("jobs");
    let ticks = jobs::Ticks::default();
    for _ in 0..2 {
        block_on_io(jobs::run(
            &registry,
            "greet",
            "step",
            &dir,
            &ticks,
            heiko_luau_plugins_host::runtime::JobRef {
                stores: registry.stores(),
                conversations: None,
            },
            Default::default(),
        ))
        .unwrap();
    }
    let store = block_on_io(
        registry
            .stores()
            .of("greet", heiko_luau_plugins_host::store::APP),
    )
    .unwrap();
    let going = block_on_io(async { store.session().await.get("going").await })
        .unwrap();
    assert_eq!(going, Some(serde_json::json!({ "n": 2 })));
}

/// The built-in `shipped`, loaded.
fn shipped() -> Loaded {
    let source =
        r#"return require("heiko").plugin { before_tool = function() end }"#;
    let files = Files {
        source: source.into(),
        ..Files::named("shipped")
    };
    block_on_io(load("shipped", files)).unwrap()
}

/// A plugin heiko comes with is active once loaded, with no Allow, even
/// one reaching what a repository plugin would wait for; the repository
/// cannot have a plugin of its name.
#[test]
fn a_built_in_plugin_is_active_and_its_name_is_its_own() {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    )
    .with_built_in(vec![shipped()]);
    block_on_io(registry.load_built_in());
    let names = |registry: &Registry| -> Vec<String> {
        block_on_io(registry.active())
            .iter()
            .map(|active| active.loaded.declaration.name.clone())
            .collect()
    };
    assert_eq!(names(&registry), ["shipped"]);
    // The repository's `greet` loads beside it; one named `shipped` does
    // not.
    let repo = ProjectRepo::init(
        home.path().join("v1"),
        Identity::default(),
        &[
            (
                "greet/heiko.toml".to_owned(),
                Manifest::text("greet", "Says hello."),
            ),
            ("greet/src/plugin.luau".to_owned(), TOOL_ONLY.to_owned()),
            (
                "shipped/heiko.toml".to_owned(),
                Manifest::text("shipped", "Ships."),
            ),
            (
                "shipped/src/plugin.luau".to_owned(),
                r#"return require("heiko").plugin { }"#.to_owned(),
            ),
        ],
    )
    .unwrap();
    let trunk = repo.trunk().unwrap();
    block_on_io(registry.reload(&Project::from(repo), &trunk));
    assert_eq!(names(&registry), ["shipped", "greet"]);
    let overview = block_on_io(registry.overview());
    let entries: Vec<(&str, bool, bool)> = overview
        .plugins
        .iter()
        .map(|entry| {
            (
                entry.name.as_str(),
                entry.built_in,
                matches!(entry.standing, Standing::Broken { .. }),
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            ("shipped", true, false),
            ("greet", false, false),
            ("shipped", false, true)
        ]
    );
}

const LISTS: &str = r#"
local heiko = require("heiko")
local ui = heiko.ui
return heiko.plugin { uses = { store = true },
  repo_nav = { label = "List", page = "list" },
  pages = {
    list = function(params, model, env)
      local rows = {}
      for _, item in ipairs(heiko.await(heiko.cmd.store.list("item/"))) do
        table.insert(rows, ui.button(item.value, "drop", { key = item.key }))
      end
      table.insert(rows, env.run.decide and "keyed" or "no key")
      return ui.stack(rows)
    end,
  },
  repo_data = function(_, model, env)
    return { nav = { detail = tostring(#heiko.await(heiko.cmd.store.list("item/"))) .. " items" } }
  end,
  actions = {
    add = function(args, model, env) heiko.give(heiko.cmd.store.put("item/" .. args.text, args.text)) end,
    drop = function(args, model, env) heiko.give(heiko.cmd.store.delete(args.key)) end,
  },
}
"#;

/// A plugin's page of a repository and what it keeps of it read its
/// store there, and whether a decider key is saved; an action from the page
/// writes it, and another repository's store is its own.
#[test]
fn a_plugins_page_of_a_repository_reads_and_writes_its_store_there() {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, LISTS, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    for text in ["milk", "eggs"] {
        block_on_io(registry.repo_action(
            "greet",
            "home",
            "add",
            serde_json::json!({ "text": text }),
            None,
            None,
        ))
        .unwrap();
    }
    let kept = block_on_io(registry.repo_data("home"));
    assert_eq!(kept.0["greet"]["nav"]["detail"], "2 items");
    let elsewhere = block_on_io(registry.repo_data("work"));
    assert_eq!(elsewhere.0["greet"]["nav"]["detail"], "0 items");

    let params = std::collections::BTreeMap::from([
        ("entry".to_owned(), "greet".to_owned()),
        ("repo".to_owned(), "home".to_owned()),
    ]);
    let page = block_on_io(registry.page(
        "greet",
        "list",
        &params,
        false,
        &Default::default(),
    ));
    let tree = page["tree"].to_string();
    assert!(tree.contains("eggs") && tree.contains("milk"), "{tree}");
    assert!(tree.contains("no key"), "{tree}");
    let keyed = block_on_io(registry.page(
        "greet",
        "list",
        &params,
        true,
        &Default::default(),
    ));
    assert!(keyed["tree"].to_string().contains("keyed"), "{keyed}");
    // Its buttons act in the repository the page shows.
    assert!(tree.contains("luau-repo/greet/drop"), "{tree}");
    block_on_io(registry.repo_action(
        "greet",
        "home",
        "drop",
        serde_json::json!({ "key": "item/milk" }),
        None,
        None,
    ))
    .unwrap();
    assert_eq!(
        block_on_io(registry.repo_data("home")).0["greet"]["nav"]["detail"],
        "1 items"
    );
}

const CLAIMS: &str = r#"
local heiko = require("heiko")
return heiko.plugin { uses = { store = true },
  actions = {
    claim = function(args, model, env) heiko.give(heiko.cmd.store.put("claim/" .. args.issue, args.run)) end,
  },
  recover = function(input, model, env)
    local dead = {}
    for _, run in ipairs(input.runs) do dead[run] = true end
    for _, claim in ipairs(heiko.await(heiko.cmd.store.list("claim/"))) do
      if dead[claim.value] then heiko.give(heiko.cmd.store.delete(claim.key)) end
    end
  end,
}
"#;

/// As heiko starts, `recover` lets go of what a plugin held for the runs
/// found cut off, in each repository, and keeps what it holds for
/// others.
#[test]
fn recover_lets_go_of_what_dead_runs_held() {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, CLAIMS, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    for (repo, issue, run) in [
        ("a", "td-1", "r-dead"),
        ("a", "td-2", "r-live"),
        ("b", "td-3", "r-dead"),
    ] {
        block_on_io(registry.repo_action(
            "greet",
            repo,
            "claim",
            serde_json::json!({ "issue": issue, "run": run }),
            None,
            None,
        ))
        .unwrap();
    }
    block_on_io(
        registry.recover(&["a".into(), "b".into()], &["r-dead".into()]),
    );
    let held = |repo: &str| -> Vec<String> {
        let kv = block_on_io(registry.stores().of("greet", repo)).unwrap();
        block_on_io(kv.everything()).unwrap().into_keys().collect()
    };
    assert_eq!(held("a"), ["claim/td-2"]);
    assert!(held("b").is_empty());
}

const APP_PAGES: &str = r#"
local heiko = require("heiko")
local ui = heiko.ui
return heiko.plugin { uses = { files = { "repository/greet" } },
  drafts = { "search" },
  nav = { label = "Greet", page = "notes" },
  pages = {
    notes = function(params, model, env)
      local names = {}
      for _, repo in ipairs(heiko.await(heiko.cmd.files.repositories())) do
        table.insert(names, repo.name .. "=" .. (repo.normalized ~= json.null and repo.normalized or "none"))
      end
      return ui.stack({
        ui.text("repo " .. tostring(params.repo)),
        ui.text("search " .. tostring((params.drafts or {}).search)),
        ui.text("listed " .. table.concat(names, ",")),
        ui.text("here " .. tostring(heiko.await(heiko.cmd.files.roots())["repository/greet"])),
        ui.link("Open", "notes", { params = { note = "a" } }),
        ui.switch("Tick", nil, true, "tick", { id = 1 }),
      })
    end,
  },
  app = function(_, model, env) return { n = 1 } end,
  setup = function(app, model, env)
    return { step = "s", title = "S", heading = "h", detail = "d",
      body = ui.text(#(env.run.picked or {}) .. " picked") }
  end,
  actions = { tick = function(args, model, env) end },
}
"#;

/// A plugin's page of the app (ADR 0050, gap L): drawn with no
/// repository, what its `drafts` hold as `params.drafts`, the
/// repositories heiko lists, its links opening its pages of the app; and
/// its setup step drawn of the repositories picked.
#[test]
fn a_plugins_page_of_the_app_reads_what_is_typed_and_the_repositories() {
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, APP_PAGES, None);
    block_on_io(registry.reload(&v1, &trunk));
    allow(&registry).unwrap();
    let repo = |name: &str, remote: Option<&str>| heiko_ui_plugin::RepoCtx {
        id: heiko_ui_plugin::RepoId(format!("id-{name}")),
        name: name.to_owned(),
        remote: remote.map(str::to_owned),
        folder: None,
        checkout: home.path().join(name),
        dir: home.path().join("heiko").join(name),
        workspaces: home.path().join("ws"),
    };
    registry.lists_repositories(&[
        repo("shop", Some("git@github.com:Acme/Shop.git")),
        repo("web", None),
    ]);
    let params = std::collections::BTreeMap::from([
        ("entry".to_owned(), "greet".to_owned()),
        ("page".to_owned(), "notes".to_owned()),
    ]);
    let drafts = std::collections::BTreeMap::from([(
        "search".to_owned(),
        "milk".to_owned(),
    )]);
    let page =
        block_on_io(registry.page("greet", "notes", &params, false, &drafts));
    let tree = page["tree"].to_string();
    assert!(tree.contains("repo nil"), "{tree}");
    assert!(tree.contains("search milk"), "{tree}");
    assert!(
        tree.contains("listed shop=github.com/Acme/Shop,web=none"),
        "{tree}"
    );
    // No repository's own folder: outside one, the plugin reaches each's.
    assert!(tree.contains("here nil"), "{tree}");
    // Its links open its pages of the app, its switches its actions.
    assert!(tree.contains(r#""page":"notes""#), "{tree}");
    assert!(!tree.contains(r#""repo":"#), "{tree}");
    assert!(tree.contains("luau-app/greet/tick"), "{tree}");

    let mut of_shop = params.clone();
    of_shop.insert("repo".into(), "shop".into());
    let shop = block_on_io(registry.page(
        "greet",
        "notes",
        &of_shop,
        false,
        &Default::default(),
    ));
    let tree = shop["tree"].to_string();
    assert!(tree.contains("repo shop"), "{tree}");
    let folder = home.path().join("heiko/shop/greet").display().to_string();
    assert!(tree.contains(&format!("here {folder}")), "{tree}");
    assert!(tree.contains("luau-repo/greet/tick"), "{tree}");

    let picked = [serde_json::json!({ "name": "acme/shop" })];
    let step = block_on_io(registry.setup("greet", &picked));
    assert!(step["tree"].to_string().contains("1 picked"), "{step}");
}

/// Actions on a run not going on, each on the model an interface folded
/// before any: every one is kept (ADR 0056). They are messages to the
/// host's one mailbox of the run, each taken on what the last left, where
/// a copy of the model each would keep one.
#[test]
fn actions_on_a_run_not_going_on_are_all_kept() {
    const SLOW: &str = r#"
local heiko = require("heiko")
return heiko.plugin { tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  actions = {
    add = function(_, model)
      -- What it awaits lets the other in, between its steps.
      heiko.await(heiko.cmd.time["local"](0))
      model.n = (model.n or 0) + 1
    end,
  },
}
"#;
    let home = tempfile::tempdir().unwrap();
    let registry = Registry::at(
        home.path().join("x"),
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    let (v1, trunk) = project(home.path(), 1, SLOW, None);
    block_on_io(registry.reload(&v1, &trunk));
    let folded = serde_json::json!({ "n": 0 });
    let act = || {
        registry.action(heiko_luau_plugins_host::registry::ActionCall {
            plugin: "greet",
            action: "add",
            args: &serde_json::Value::Null,
            run: "r1",
            repo: None,
            state: &folded,
            settings: None,
            chats: None,
            decide: None,
            live: false,
            keyed: false,
        })
    };
    let (first, second) =
        block_on_io(async { futures_util::join!(act(), act()) });
    assert_eq!((first.error.clone(), second.error.clone()), (None, None));
    let mut left = [first.state["n"].clone(), second.state["n"].clone()];
    left.sort_by_key(|n| n.as_i64());
    assert_eq!(left, [serde_json::json!(1), serde_json::json!(2)]);
    assert!(first.changed && second.changed);
    // One after the other, an interface still showing the first model:
    // the host's model is the one the action goes on.
    let third = block_on_io(act());
    assert_eq!(third.state["n"], serde_json::json!(3));
}
