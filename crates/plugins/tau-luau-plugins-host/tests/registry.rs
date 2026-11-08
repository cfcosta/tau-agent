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

use tau_luau_plugins::Standing;
use tau_luau_plugins_host::registry::Registry;
use tau_testing::block_on_io;
use tau_vcs_host::{Identity, Project, ProjectRepo};

const TOOL_ONLY: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "greet",
  description = "Says hello.",
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
}
"#;

const BLOCKING: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "greet",
  description = "Says hello, and blocks rm.",
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  before_tool = function(call, ctx)
    if call.name == "exec_command" then return tau.block("no") end
  end,
}
"#;

const PASSING: &str = r#"
local tau = require("tau")
local t = tau.test
t.case("says hi", function() t.equal(t.run{}:tool("hello"), "hi") end)
"#;

const FAILING: &str = r#"
local tau = require("tau")
local t = tau.test
t.case("says bye", function() t.equal(t.run{}:tool("hello"), "bye") end)
"#;

/// A project whose trunk holds the plugin `greet` with `plugin` and,
/// when given, a test file.
fn project(
    home: &std::path::Path,
    n: usize,
    plugin: &str,
    test: Option<&str>,
) -> (Project, String) {
    let mut files = vec![("greet/plugin.luau".to_owned(), plugin.to_owned())];
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

    // Allowed, it activates; read again, it stays active.
    block_on_io(registry.allow("greet")).unwrap();
    assert_eq!(standing(&registry), Standing::Active);
    assert_eq!(
        active_description(&registry).as_deref(),
        Some("Says hello, and blocks rm.")
    );
    block_on_io(registry.reload(&v2, &trunk));
    assert_eq!(standing(&registry), Standing::Active);
    assert!(
        block_on_io(registry.allow("greet")).is_err(),
        "nothing waits"
    );

    // Its tests fail: the version before stays.
    let (v3, trunk) = project(home.path(), 3, BLOCKING, Some(FAILING));
    block_on_io(registry.reload(&v3, &trunk));
    assert_eq!(standing(&registry), Standing::Failing);
    assert_eq!(
        active_description(&registry).as_deref(),
        Some("Says hello, and blocks rm.")
    );

    // It does not load: the version before stays, and the error shows.
    let (v4, trunk) = project(home.path(), 4, "return tau.plugin {", None);
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
    block_on_io(first.allow("greet")).unwrap();

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
local tau = require("tau")
local ui = tau.ui
return tau.plugin {
  name = "greet",
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  settings = {
    schema = { type = "object", properties = { loud = { type = "boolean" } } },
    default = { loud = false },
    view = function(settings, ctx)
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
    std::fs::write(folder.join("plugin.luau"), plugin).unwrap();
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
local tau = require("tau")
local ui = tau.ui
return tau.plugin {
  name = "greet",
  tools = { hello = { description = "Hello.", call = function() return "hi" end } },
  settings = {
    schema = { type = "object", properties = { step = { type = "integer" } } },
    default = { step = 1 },
  },
  view = function(state, ctx)
    return { status = ui.status(tostring(state.n or 0)) }
  end,
  actions = {
    add = function(args, ctx)
      ctx.state.n = (ctx.state.n or 0) + ctx.settings.step * (args.times or 1)
      ctx.log("added")
    end,
    broken = function(args, ctx) error("no luck") end,
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
            tau_luau_plugins_host::registry::ActionCall {
                plugin: "greet",
                action,
                args: &serde_json::json!({ "times": 2 }),
                run: "r1",
                repo: None,
                state: &state,
                settings: Some(&serde_json::json!({ "step": 5 })),
                chats: None,
                jev: None,
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
local tau = require("tau")
return tau.plugin {
  name = "greet",
  description = "Titles runs, and keeps the tree's rewrites.",
  fills = { "tau.title", "tau.compaction" },
  title = function(run, ctx)
    return "Asked: " .. run.task
  end,
  compaction = { rewriters = { "tau-tree-compaction" } },
}
"#;

/// A plugin that fills slots waits for the person, as any new reach does;
/// once allowed, tau's stand-ins ask it: a run's title from its `title`,
/// cut to a title's length, and the rewriters it names beside the Luau
/// plugins' own. Before, they answer as an empty filler would.
#[test]
fn a_plugin_fills_the_title_and_compaction_slots() {
    use tau_luau_plugins_host::slots::{LuauCompaction, LuauTitle};
    use tau_ui_plugin::{compaction::Compaction, title::Title};
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
            ["filling `tau.title`", "filling `tau.compaction`"]
        ),
        other => panic!("{other:?}"),
    }
    assert_eq!(block_on_io(title.write("fix it", "m")).unwrap(), None);
    assert_eq!(compaction.rewriters(), [tau_luau_plugins::NAME]);

    block_on_io(registry.allow("greet")).unwrap();
    assert_eq!(
        block_on_io(title.write("fix it", "m")).unwrap().as_deref(),
        Some("Asked: fix it")
    );
    let long = "x".repeat(100);
    let written = block_on_io(title.write(&long, "m")).unwrap().unwrap();
    assert_eq!(written.chars().count(), tau_ui_plugin::title::MAX_CHARS);
    assert_eq!(
        compaction.rewriters(),
        [tau_luau_plugins::NAME, "tau-tree-compaction"]
    );
}

/// A plugin fills only the slots a Luau plugin may, and only with what
/// each needs.
#[test]
fn a_plugin_fills_only_what_it_may() {
    use tau_luau_plugins_host::runtime::{Files, load};
    let refused = |source: &str| {
        let files = Files {
            source: source.into(),
            ..Files::default()
        };
        block_on_io(load("greet", files)).unwrap_err()
    };
    let vcs = refused(
        r#"return require("tau").plugin { name = "greet", fills = { "tau.vcs" } }"#,
    );
    assert!(vcs.contains("`tau.vcs`"), "{vcs}");
    let no_hook = refused(
        r#"return require("tau").plugin { name = "greet", fills = { "tau.title" } }"#,
    );
    assert!(no_hook.contains("a `title` hook"), "{no_hook}");
}

/// A plugin that fills `tau.notify` sends each notice where it reaches:
/// here, a server on this computer that hears the notice's words.
#[test]
fn a_plugin_sends_tau_s_notices_on() {
    use std::io::{Read, Write};

    use tau_luau_plugins_host::slots::LuauNotify;
    use tau_ui_plugin::notify::{Notice, Notify};
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
local tau = require("tau")
return tau.plugin {{
  name = "greet",
  uses = {{ network = {{ "127.0.0.1" }} }},
  fills = {{ "tau.notify" }},
  notify = function(notice, ctx)
    http.request({{ method = "POST", url = "http://127.0.0.1:{port}/", body = notice.title .. ": " .. notice.body }})
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
    block_on_io(registry.allow("greet")).unwrap();
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
local tau = require("tau")
return tau.plugin {
  name = "greet",
  uses = { secrets = { "forge-token" } },
  fills = { "tau.forge" },
  forge = {
    name = "Gitea at home",
    id = "home",
    host = "git.home.lan",
    token = "forge-token",
    account = function(input, ctx) return "cainã" end,
    repos = function(input, ctx)
      return { { name = "me/tau", description = "the agent", branch = "trunk" } }
    end,
    can_push = function(input, ctx) return input.repo == "me/tau" end,
    checks = function(input, ctx) return "passed" end,
  },
}
"#;

/// A plugin that fills `tau.forge` is the forge tau clones from and
/// pushes to: it is named and placed by its `forge` table, signed in by
/// the secret it names, which only tau's remote reads, and asked the
/// rest through its hooks.
#[test]
fn a_plugin_fills_the_forge_slot() {
    use tau_luau_plugins_host::slots::LuauForge;
    use tau_ui_plugin::forge::{Checks, Forge};
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
    block_on_io(registry.allow("greet")).unwrap();
    let forge = LuauForge(registry.clone());
    assert_eq!((forge.name(), forge.id()), ("Gitea at home", "home"));
    assert_eq!(forge.clone_url("me/tau"), "https://git.home.lan/me/tau.git");
    assert_eq!(
        forge
            .repository_of("git@git.home.lan:me/tau.git")
            .as_deref(),
        Some("me/tau")
    );
    assert_eq!(forge.repository_of("https://github.com/me/tau"), None);

    // Signed out until its secret is set.
    assert_eq!(block_on_io(forge.signed_in()), None);
    let error = block_on_io(forge.remote("me/tau")).unwrap_err().to_string();
    assert!(error.contains("forge-token"), "{error}");
    block_on_io(registry.set_secret("greet", "forge-token", "t0k3n")).unwrap();
    assert_eq!(block_on_io(forge.signed_in()).as_deref(), Some("cainã"));
    let remote = block_on_io(forge.remote("me/tau")).unwrap();
    assert_eq!(remote.token.as_deref(), Some("t0k3n"));
    // A secret it does not name is refused.
    assert!(block_on_io(registry.set_secret("greet", "github", "x")).is_err());

    let repos = block_on_io(forge.repos()).unwrap();
    assert_eq!(repos[0].name, "me/tau");
    assert_eq!(repos[0].branch, "trunk");
    assert!(block_on_io(forge.can_push("me/tau")).unwrap());
    assert!(!block_on_io(forge.can_push("you/x")).unwrap());
    assert_eq!(
        block_on_io(forge.checks("me/tau", "abc")).unwrap(),
        Checks::Passed
    );
    // A hook it lacks says so.
    let pull = tau_ui_plugin::forge::OpenPull {
        repo: "me/tau".into(),
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
    use tau_luau_plugins_host::slots::repository_of;
    for url in [
        "https://git.home.lan/me/tau",
        "https://git.home.lan/me/tau.git",
        "https://git.home.lan/me/tau/",
        "ssh://git@git.home.lan/me/tau.git",
        "git@git.home.lan:me/tau.git",
    ] {
        assert_eq!(
            repository_of("git.home.lan", url).as_deref(),
            Some("me/tau"),
            "{url}"
        );
    }
    for url in [
        "https://git.home.lan.evil/me/tau",
        "https://git.home.lan/me",
        "https://git.home.lan/me/tau/extra",
        "git@other:me/tau.git",
    ] {
        assert_eq!(repository_of("git.home.lan", url), None, "{url}");
    }
}

const ASKS_THE_MODEL: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "greet",
  uses = { infer = true },
  fills = { "tau.title" },
  title = function(run, ctx)
    local answer = tools.infer({ task = "A title for this task", context = run.task })
    if answer.ok then return answer.value end
  end,
}
"#;

/// A hook outside any run asks the person's model with `infer`: the
/// title hook, with the run's own model, as a run's `infer` answers.
#[test]
fn a_title_hook_asks_the_persons_model() {
    use tau_luau_plugins_host::slots::LuauTitle;
    use tau_ui_plugin::title::Title;
    let home = tempfile::tempdir().unwrap();
    let llm = tau_testing::scripted::ScriptedModel::new()
        .turn(|t| t.text("Fix the login"));
    let store = block_on_io(tau_store_sqlite::memory()).unwrap();
    let reached = llm.clone();
    let model = tau_ui_plugin::Model::new(
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
    block_on_io(registry.allow("greet")).unwrap();
    let title =
        block_on_io(LuauTitle(registry).write("fix the login bug", "m-run"))
            .unwrap();
    assert_eq!(title.as_deref(), Some("Fix the login"));
    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].settings.model, "m-run");
}

const COUNTS: &str = r#"
local tau = require("tau")
return tau.plugin {
  name = "greet",
  jobs = {
    count = {
      every = 3600,
      label = "Counts",
      run = function(ctx) ctx.state.n = (ctx.state.n or 0) + 1 end,
    },
  },
}
"#;

/// A plugin's job is due once its time came, keeps what its `ctx.state`
/// left from run to run, and once run waits for its next time. A new job
/// waits for the person's Allow, as any new reach does.
#[test]
fn a_plugins_job_runs_when_due_and_keeps_its_state() {
    use tau_luau_plugins_host::jobs;
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
    block_on_io(registry.allow("greet")).unwrap();
    let dir = home.path().join("jobs");
    let due = block_on_io(jobs::due_now(&registry, &dir));
    assert_eq!(due, [("greet".into(), "count".into(), "Counts".into())]);
    for _ in 0..2 {
        block_on_io(jobs::run(
            &registry,
            "greet",
            "count",
            &dir,
            Default::default(),
        ))
        .unwrap();
    }
    let kept: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("greet.count.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept["state"], serde_json::json!({ "n": 2 }));
    assert!(block_on_io(jobs::due_now(&registry, &dir)).is_empty());
}

/// A job is due once `every` seconds passed since it last ran, and not
/// before; one that never ran is due.
#[test]
fn a_job_is_due_after_its_interval() {
    use tau_luau_plugins_host::jobs::due;
    assert!(due(0, 60, 60));
    assert!(due(100, 60, 160));
    assert!(!due(100, 60, 159));
    assert!(!due(100, 60, 50), "a clock that went back waits");
}

/// A job that runs more often than every minute keeps its plugin from
/// loading.
#[test]
fn a_job_runs_at_most_every_minute() {
    use tau_luau_plugins_host::runtime::{Files, load};
    let source = r#"return require("tau").plugin { name = "greet",
      jobs = { fast = { every = 5, run = function() end } } }"#;
    let files = Files {
        source: source.into(),
        ..Files::default()
    };
    let error = block_on_io(load("greet", files)).unwrap_err();
    assert!(error.contains("`fast`"), "{error}");
}

const BUILT_IN: &[tau_luau_plugins_host::registry::BuiltIn] = &[
    tau_luau_plugins_host::registry::BuiltIn {
        name: "shipped",
        files: &[(
            "plugin.luau",
            r#"return require("tau").plugin { name = "shipped", before_tool = function() end }"#,
        )],
    },
];

/// A plugin tau comes with is active once loaded, with no Allow, even
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
    .with_built_in(BUILT_IN);
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
            ("greet/plugin.luau".to_owned(), TOOL_ONLY.to_owned()),
            (
                "shipped/plugin.luau".to_owned(),
                r#"return require("tau").plugin { name = "shipped" }"#
                    .to_owned(),
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
local tau = require("tau")
local ui = tau.ui
return tau.plugin {
  name = "greet",
  uses = { store = true },
  repo_nav = { label = "List", page = "list" },
  pages = {
    list = function(params, ctx)
      local rows = {}
      for _, item in ipairs(ctx.store.list("item/")) do
        table.insert(rows, ui.button(item.value, "drop", { key = item.key }))
      end
      return ui.stack(rows)
    end,
  },
  repo_data = function(ctx)
    return { nav = { detail = tostring(#ctx.store.list("item/")) .. " items" } }
  end,
  actions = {
    add = function(args, ctx) ctx.store.put("item/" .. args.text, args.text) end,
    drop = function(args, ctx) ctx.store.delete(args.key) end,
  },
}
"#;

/// A plugin's page of a repository and what it keeps of it read its
/// store there; an action from the page writes it, and another
/// repository's store is its own.
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
    block_on_io(registry.allow("greet")).unwrap();
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
    let page = block_on_io(registry.page("greet", "list", &params));
    let tree = page["tree"].to_string();
    assert!(tree.contains("eggs") && tree.contains("milk"), "{tree}");
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
local tau = require("tau")
return tau.plugin {
  name = "greet",
  uses = { store = true },
  actions = {
    claim = function(args, ctx) ctx.store.put("claim/" .. args.issue, args.run) end,
  },
  recover = function(input, ctx)
    local dead = {}
    for _, run in ipairs(input.runs) do dead[run] = true end
    for _, claim in ipairs(ctx.store.list("claim/")) do
      if dead[claim.value] then ctx.store.delete(claim.key) end
    end
  end,
}
"#;

/// As tau starts, `recover` lets go of what a plugin held for the runs
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
    block_on_io(registry.allow("greet")).unwrap();
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
