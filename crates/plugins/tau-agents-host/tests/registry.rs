//! Each agent's versions, read from its trunk (ADR 0037): a passing
//! version is live at once; one that fails leaves the one before live
//! and tells the agent, once.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use tau_agents::{
    TriggerKind,
    event::{Event, Kind},
};
use tau_agents_host::{
    clock::FakeClock,
    registry::{Registry, VERSION_TRIGGER},
};
use tau_testing::block_on_io;
use tau_vcs_host::{Identity, Project, ProjectRepo};

const EVENING: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "greet",
  description = "Says good evening.",
  triggers = { evening = { at = "18:00" }, hello = { command = "hello" } },
}
"#;

const HOURLY: &str = r#"
local tau = require("tau")
return tau.agent {
  name = "greet",
  triggers = { hourly = { every = "1h" } },
}
"#;

const PASSING: &str = r#"
local tau = require("tau")
local t = tau.test
t.case("it has triggers", function() t.equal(#t.agent({}):describe().triggers > 0, true) end)
"#;

const FAILING: &str = r#"
local tau = require("tau")
local t = tau.test
t.case("it has none", function() t.equal(#t.agent({}):describe().triggers, 0) end)
"#;

/// A repository for the agent `greet` whose trunk holds `agent` and,
/// when given, a test.
fn project(
    home: &Path,
    n: usize,
    agent: Option<&str>,
    test: Option<&str>,
) -> (Project, String) {
    let mut files = vec![(
        "AGENTS.md".to_owned(),
        "# greet\n\nGreets people.\n".to_owned(),
    )];
    if let Some(agent) = agent {
        files.push(("agent.luau".to_owned(), agent.to_owned()));
    }
    if let Some(test) = test {
        files.push(("tests/basic.luau".to_owned(), test.to_owned()));
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

/// What a registry told agents, by agent.
type Told = Arc<Mutex<Vec<(String, Event)>>>;

/// A registry over `home`, and what it told agents.
fn open(home: &Path) -> (Registry, Told) {
    let told = Arc::new(Mutex::new(Vec::new()));
    let sink = told.clone();
    let registry = Registry::at(
        home.join("agents"),
        home.join("versions.json"),
        Arc::new(FakeClock::new(1_000, 0)),
        || {},
        move |agent, event| {
            sink.lock().unwrap().push((agent.to_owned(), event));
        },
    );
    (registry, told)
}

fn live_triggers(registry: &Registry) -> Vec<String> {
    block_on_io(registry.live("greet"))
        .map(|live| {
            live.declared
                .triggers
                .iter()
                .map(|trigger| trigger.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_passing_version_is_live_and_a_failing_one_keeps_the_one_before() {
    let home = tempfile::tempdir().unwrap();
    let (registry, told) = open(home.path());

    let (v1, trunk1) = project(home.path(), 1, Some(EVENING), Some(PASSING));
    block_on_io(registry.reload("greet", &v1, &trunk1));
    assert_eq!(live_triggers(&registry), ["evening", "hello"]);
    let read = block_on_io(registry.agents())["greet"].clone();
    assert_eq!(read.purpose, "Says good evening.");
    assert_eq!(read.error, None);
    assert!(read.versions[0].live);
    assert_eq!(
        block_on_io(registry.live("greet"))
            .unwrap()
            .declared
            .triggers[1]
            .kind,
        TriggerKind::Command {
            command: "hello".into(),
            hint: None
        }
    );
    assert!(told.lock().unwrap().is_empty());

    // Its tests fail: the version before stays live, and it hears why.
    let (v2, trunk2) = project(home.path(), 2, Some(HOURLY), Some(FAILING));
    block_on_io(registry.reload("greet", &v2, &trunk2));
    assert_eq!(live_triggers(&registry), ["evening", "hello"]);
    let read = block_on_io(registry.agents())["greet"].clone();
    assert_eq!(read.error, None, "a version is live");
    assert_eq!(read.versions.len(), 2);
    assert!(!read.versions[0].live);
    assert_eq!(read.versions[0].error.as_deref(), Some("1 test fails"));
    assert!(read.versions[1].live);
    let events = told.lock().unwrap().clone();
    assert_eq!(events.len(), 1);
    let (agent, event) = &events[0];
    assert_eq!(agent, "greet");
    assert_eq!(event.kind, Kind::Version);
    assert_eq!(event.trigger, VERSION_TRIGGER);
    assert_eq!(event.at, 1_000);
    assert_eq!(event.data["failing"][0]["case"], "it has none");

    // Read again, it is not told again.
    block_on_io(registry.reload("greet", &v2, &trunk2));
    assert_eq!(told.lock().unwrap().len(), 1);

    // A passing version takes over.
    let (v3, trunk3) = project(home.path(), 3, Some(HOURLY), Some(PASSING));
    block_on_io(registry.reload("greet", &v3, &trunk3));
    assert_eq!(live_triggers(&registry), ["hourly"]);
}

#[test]
fn a_version_that_does_not_load_says_why() {
    let home = tempfile::tempdir().unwrap();
    let (registry, told) = open(home.path());

    let (v1, trunk) = project(home.path(), 1, Some("return tau.agent {"), None);
    block_on_io(registry.reload("greet", &v1, &trunk));
    let read = block_on_io(registry.agents())["greet"].clone();
    assert!(read.live.is_none());
    assert!(read.error.is_some(), "{read:?}");
    assert_eq!(told.lock().unwrap().len(), 1);

    // Its name must be its repository's.
    let other = EVENING.replace("\"greet\"", "\"other\"");
    let (v2, trunk) = project(home.path(), 2, Some(&other), None);
    block_on_io(registry.reload("greet", &v2, &trunk));
    let error = block_on_io(registry.agents())["greet"]
        .error
        .clone()
        .unwrap();
    assert!(error.contains("its repository is `greet`"), "{error}");

    // A restart does not tell it again about a version it heard of.
    let (restarted, told_again) = open(home.path());
    block_on_io(restarted.reload("greet", &v2, &trunk));
    assert!(told_again.lock().unwrap().is_empty());
}

#[test]
fn an_agent_with_no_agent_luau_yet_is_waiting() {
    let home = tempfile::tempdir().unwrap();
    let (registry, told) = open(home.path());
    let (v1, trunk) = project(home.path(), 1, None, None);
    block_on_io(registry.reload("greet", &v1, &trunk));
    let read = block_on_io(registry.agents())["greet"].clone();
    assert!(read.live.is_none());
    assert_eq!(read.error, None);
    assert!(read.versions.is_empty());
    assert_eq!(read.purpose, "Greets people.");
    assert!(told.lock().unwrap().is_empty());
}

#[test]
fn a_scan_reads_every_agent_in_its_directory() {
    let home = tempfile::tempdir().unwrap();
    let (registry, _) = open(home.path());
    let agents = home.path().join("agents");
    ProjectRepo::init(
        agents.join("greet"),
        Identity::default(),
        &[
            ("agent.luau".to_owned(), EVENING.to_owned()),
            ("tests/basic.luau".to_owned(), PASSING.to_owned()),
        ],
    )
    .unwrap();
    // Not an agent's name: left alone.
    std::fs::create_dir_all(agents.join("Not An Agent")).unwrap();
    block_on_io(registry.scan());
    let names: Vec<String> =
        block_on_io(registry.agents()).into_keys().collect();
    assert_eq!(names, ["greet"]);
    assert_eq!(live_triggers(&registry), ["evening", "hello"]);

    std::fs::remove_dir_all(agents.join("greet")).unwrap();
    block_on_io(registry.scan());
    assert!(block_on_io(registry.agents()).is_empty());
}
