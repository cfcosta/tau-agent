//! A repository the jj CLI of jj-lib's version made or last wrote
//! opens and works in tau, and one tau made or last wrote works in the
//! CLI: the two take turns committing in one workspace, each loading
//! what the other left (the repository, the operation log and the
//! working copy's state), and both read the same history, the one the
//! model says.
//!
//! Where that CLI is not installed this test says it did not run. Each
//! world lives in a temporary directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
use tau_testing::block_on_io as block_on;
use tau_vcs_host::{Identity, JjEnv, Vcs, VcsPlugin};

#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::PrettyPrintable)]
enum Writer {
    Tau,
    Cli,
}

/// Who makes the repository: tau (`Vcs::init`, an internal Git store)
/// or the CLI (`jj git init`, colocated or not).
#[derive(Debug, Clone, Copy, hegel::PrettyPrintable)]
enum Made {
    Tau,
    Cli { colocate: bool },
}

/// One turn: `writer` writes `text` to `file` and commits it as
/// `message`.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Turn {
    writer: Writer,
    file: String,
    text: String,
    message: String,
}

#[hegel::composite]
fn turn(tc: &TestCase) -> Turn {
    Turn {
        writer: tc.draw(gs::sampled_from(vec![Writer::Tau, Writer::Cli])),
        file: tc
            .draw(gs::sampled_from(vec!["a.txt", "b/c.txt", "d.rs"]))
            .to_owned(),
        text: tc.draw(gs::text().max_size(20).alphabet("ab\n é语")),
        message: tc.draw(gs::from_regex("[a-z]{1,8}").fullmatch(true)),
    }
}

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        Self { _dir: dir, root }
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn identity(&self) -> Identity {
        Identity {
            name: "Tau".into(),
            email: "tau@localhost".into(),
            env: JjEnv {
                config_dir: Some(self.root.join("config")),
                home_dir: Some(self.root.join("home")),
                system_dir: None,
                hostname: Some("desk".into()),
                username: Some("person".into()),
                vars: HashMap::new(),
            },
        }
    }

    /// The CLI in this world: its home and config directory, the same
    /// author as tau's, and no other variable of this process.
    fn cli(&self, jj: &Path) -> Command {
        let mut command = Command::new(jj);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("JJ_USER", "Tau")
            .env("JJ_EMAIL", "tau@localhost")
            .current_dir(self.root.join("work"));
        command
    }

    fn run(&self, jj: &Path, args: &[&str]) -> String {
        let output = self.cli(jj).args(args).output().unwrap();
        assert!(output.status.success(), "jj {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }
}

fn call(
    tools: &VcsPlugin,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let tool = tools
        .tools()
        .into_iter()
        .find(|tool| tool.name() == name)
        .unwrap();
    block_on(tool.call(args, ToolCtx::detached()))
        .map(|output| output.details.unwrap())
        .map_err(|error| error.to_string())
}

/// The workspace's history, newest first, as tau's `vcs_log` shows it
/// in a session that opens the workspace afresh: each change's
/// description, `@` (empty, undescribed) left out.
fn tau_history(world: &World) -> Vec<String> {
    let vcs = block_on(Vcs::open(world.work(), world.identity())).unwrap();
    let log =
        call(&VcsPlugin::new(vcs), "vcs_log", json!({ "limit": 100 })).unwrap();
    log["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|change| change["working_copy"] != true)
        .map(|change| change["description"].as_str().unwrap().to_owned())
        .collect()
}

/// The same history as the CLI shows it.
fn cli_history(world: &World, jj: &Path) -> Vec<String> {
    world
        .run(
            jj,
            &[
                "log",
                "--no-graph",
                "-r",
                "::@- ~ root()",
                "-T",
                "description ++ \"\\0\"",
            ],
        )
        .split_terminator('\0')
        .map(str::to_owned)
        .collect()
}

/// Turns of tau and the CLI on one workspace: each loads what the other
/// wrote, and both read the history the turns made.
#[hegel::test(test_cases = 25)]
fn tau_and_the_jj_cli_read_each_others_repositories(tc: TestCase) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let made = if tc.draw(gs::booleans()) {
        Made::Tau
    } else {
        Made::Cli {
            colocate: tc.draw(gs::booleans()),
        }
    };
    let turns: Vec<Turn> = tc.draw(gs::vecs(turn()).min_size(1).max_size(5));
    let world = World::new();
    match made {
        Made::Tau => {
            block_on(Vcs::init(world.work(), world.identity())).unwrap();
        }
        Made::Cli { colocate } => {
            std::fs::create_dir_all(world.work()).unwrap();
            let flag = if colocate {
                "--colocate"
            } else {
                "--no-colocate"
            };
            world.run(&jj, &["git", "init", flag]);
        }
    }
    let mut model: Vec<String> = Vec::new();
    for turn in &turns {
        let path = world.work().join(&turn.file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &turn.text).unwrap();
        match turn.writer {
            Writer::Tau => {
                let vcs = block_on(Vcs::open(world.work(), world.identity()))
                    .unwrap();
                call(
                    &VcsPlugin::new(vcs),
                    "vcs_commit",
                    json!({ "message": turn.message }),
                )
                .unwrap();
            }
            Writer::Cli => {
                world.run(&jj, &["commit", "-m", &turn.message]);
            }
        }
        // jj stores a description with one trailing newline.
        model.insert(0, format!("{}\n", turn.message));
        // Each reads the other's latest write before the next turn.
        assert_eq!(tau_history(&world), model, "tau after {turn:?}");
        assert_eq!(cli_history(&world, &jj), model, "the CLI after {turn:?}");
    }
    // The CLI loads tau's operations and working-copy state whole.
    world.run(&jj, &["op", "log", "--no-graph", "-T", "id ++ \"\\n\""]);
    world.run(&jj, &["status"]);
    world.run(&jj, &["debug", "index"]);
}
