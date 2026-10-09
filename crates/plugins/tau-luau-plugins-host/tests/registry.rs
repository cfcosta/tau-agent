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
    if call.name == "bash" then return tau.block("no") end
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
    let first = Registry::at(home.path().join("x"), allowed.clone(), || {});
    block_on_io(first.reload(&v1, &trunk));
    assert!(matches!(standing(&first), Standing::Waiting { .. }));
    block_on_io(first.allow("greet")).unwrap();

    let second = Registry::at(home.path().join("x"), allowed, || {});
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
        None => std::fs::remove_file(folder.join("tests/basic.luau"))
            .or(Ok(())),
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
    let registry =
        Registry::at(home.path().join("none"), home.path().join("allowed.json"), || {});
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
