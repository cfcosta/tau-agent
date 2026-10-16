//! The skill's worked agents load and pass their tests: what the skill
//! teaches is what the host reads (ADR 0037).

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{sync::Arc, time::Duration};

use tau_agents_host::{
    AGENT,
    skill::{self, EXAMPLES},
};
use tau_luau_plugins_host::runtime::{Files, evaluate, run_tests};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

#[test]
fn the_skills_agents_pass_their_tests() {
    for (name, files) in EXAMPLES {
        let paths: Vec<(String, Vec<u8>)> = files
            .iter()
            .map(|(path, source)| {
                ((*path).to_owned(), source.as_bytes().to_vec())
            })
            .collect();
        let files = Files::from_paths(&paths, AGENT).unwrap();
        let modules = Arc::new(
            files
                .modules()
                .unwrap_or_else(|error| panic!("{name}: {error}")),
        );
        // It declares what the host reads.
        let described = block_on_io(evaluate(
            name,
            modules.clone(),
            "local tau = require(\"tau\")\n\
             return tau._describe_agent(require(\"agent\"))\n"
                .into(),
            Duration::from_secs(2),
            CancellationToken::new(),
        ))
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(described["name"], name);
        let tests = block_on_io(run_tests(
            name,
            modules,
            &files,
            CancellationToken::new(),
        ));
        assert!(tests.len() >= 3, "{name}: {tests:?}");
        for test in tests {
            assert!(test.passed, "{name}: {test:?}");
        }
    }
}

/// The skill installs as `SKILL.md`, rendered with its worked agents
/// inlined, and the agents' files to copy from; installing again replaces
/// what was there, a file the skill no longer has included.
#[test]
fn the_skill_installs_where_skills_are_read() {
    let dir = tempfile::tempdir().unwrap();
    skill::install(dir.path()).unwrap();
    let folder = dir.path().join(skill::NAME);
    let stale = folder.join("examples").join("gone").join("agent.luau");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, "-- an example the skill dropped").unwrap();
    skill::install(dir.path()).unwrap();
    assert!(std::fs::symlink_metadata(&stale).is_err(), "left behind");
    let text = std::fs::read_to_string(folder.join("SKILL.md")).unwrap();
    assert!(text.starts_with("---\nname: tau-agents\n"));
    assert_eq!(text, skill::render());
    for (name, files) in EXAMPLES {
        for (path, source) in files {
            let copied = folder.join("examples").join(name).join(path);
            assert_eq!(std::fs::read_to_string(copied).unwrap(), *source);
        }
    }
}
