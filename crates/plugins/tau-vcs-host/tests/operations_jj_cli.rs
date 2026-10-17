//! tau's operations against the jj CLI's in one repository
//! (`docs/reference/vcs.md`, "Tagged operations"; ADR 0043, decision
//! 6): tau's main chat and the jj CLI of jj-lib's version take turns in
//! a project's main workspace, the CLI making changes, describing them,
//! setting bookmarks and snapshotting, tau running its tools and adding
//! and forgetting run workspaces. The operations each turn wrote are
//! exactly the ones `is_taus` says are, or are not, tau's, and
//! `jj op log` describes each of tau's `tau: …` and none of the CLI's.
//!
//! Where that CLI is not installed this test says it did not run. Each
//! world lives in a temporary directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;
#[path = "common/ops.rs"]
mod ops;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use hegel::{TestCase, generators as gs};
use jj_lib::object_id::ObjectId as _;
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin as _,
    tool::{AgentTool, ToolCtx},
};
use tau_testing::block_on_io as block_on;
use tau_vcs_host::{
    DESCRIPTION_PREFIX,
    Identity,
    JjEnv,
    ProjectRepo,
    Vcs,
    VcsPlugin,
    is_taus,
};

/// One turn of tau's.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Tau {
    /// A file written, then `vcs_status`, which snapshots it.
    Write {
        file: String,
        text: String,
    },
    New,
    Describe(String),
    Commit(String),
    /// A run's workspace, added on trunk.
    AddRun,
    /// The newest run's workspace, forgotten.
    ForgetRun,
}

/// One turn of the jj CLI's.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Cli {
    /// A file written, then `jj status`, which snapshots it.
    Write {
        file: String,
        text: String,
    },
    New,
    Describe(String),
    /// `jj bookmark set <name> -r @`.
    Bookmark(String),
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
enum Turn {
    Tau(Tau),
    Cli(Cli),
}

#[hegel::composite]
fn turn(tc: &TestCase) -> Turn {
    let file = || {
        tc.draw(gs::sampled_from(vec!["a.txt", "b/c.txt"]))
            .to_owned()
    };
    let text = || tc.draw(gs::from_regex("[a-z]{1,6}\n").fullmatch(true));
    let message = || tc.draw(gs::from_regex("[a-z]{1,8}").fullmatch(true));
    if tc.draw(gs::booleans()) {
        Turn::Tau(match tc.draw(gs::integers::<u8>().max_value(5)) {
            0 => Tau::Write {
                file: file(),
                text: text(),
            },
            1 => Tau::New,
            2 => Tau::Describe(message()),
            3 => Tau::Commit(message()),
            4 => Tau::AddRun,
            _ => Tau::ForgetRun,
        })
    } else {
        Turn::Cli(match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => Cli::Write {
                file: file(),
                text: text(),
            },
            1 => Cli::New,
            2 => Cli::Describe(message()),
            _ => Cli::Bookmark(
                tc.draw(gs::sampled_from(vec!["b1", "b2"])).to_owned(),
            ),
        })
    }
}

/// A project tau made, its main chat's tools, and the jj CLI's world.
struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: ProjectRepo,
    tools: Vec<Arc<dyn AgentTool>>,
    runs: Vec<String>,
    made: usize,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        let identity = Identity {
            name: "Tau".into(),
            email: "tau@localhost".into(),
            env: JjEnv {
                config_dir: Some(root.join("config")),
                home_dir: Some(root.join("home")),
                system_dir: None,
                hostname: Some("desk".into()),
                username: Some("person".into()),
                vars: HashMap::new(),
            },
        };
        let project = ProjectRepo::init(
            root.join("p"),
            identity.clone(),
            &[("a.txt".into(), "one\n".into())],
        )
        .unwrap();
        let vcs = block_on(Vcs::open(project.main_dir().to_owned(), identity))
            .unwrap();
        let tools = VcsPlugin::new(vcs).tools();
        Self {
            _dir: dir,
            root,
            project,
            tools,
            runs: Vec::new(),
            made: 0,
        }
    }

    fn main(&self) -> PathBuf {
        self.project.main_dir().to_owned()
    }

    fn call(&self, name: &str, args: Value) {
        let tool = self.tools.iter().find(|tool| tool.name() == name).unwrap();
        // A refusal (nothing to commit) is a turn too: what it wrote,
        // its snapshot, is still tau's.
        let _ = block_on(tool.call(args, ToolCtx::detached()));
    }

    /// The CLI in this world, in the main workspace: its home and config
    /// directory, and no other variable of this process.
    fn cli(&self, jj: &Path, args: &[&str]) -> String {
        let output = Command::new(jj)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("JJ_USER", "Person")
            .env("JJ_EMAIL", "person@localhost")
            .current_dir(self.main())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "jj {args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn tau(&mut self, turn: &Tau) {
        match turn {
            Tau::Write { file, text } => {
                write(&self.main(), file, text);
                self.call("vcs_status", json!({}));
            }
            Tau::New => self.call("vcs_new", json!({})),
            Tau::Describe(message) => {
                self.call("vcs_describe", json!({ "message": message }));
            }
            Tau::Commit(message) => {
                self.call("vcs_commit", json!({ "message": message }));
            }
            Tau::AddRun => {
                self.made += 1;
                let name = format!("r{}", self.made);
                let trunk = self.project.trunk().unwrap();
                self.project.add_workspace(&name, &trunk).unwrap();
                self.runs.push(name);
            }
            Tau::ForgetRun => {
                if let Some(name) = self.runs.pop() {
                    self.project.forget_workspace(&name).unwrap();
                }
            }
        }
    }

    fn jj(&self, jj: &Path, turn: &Cli) {
        match turn {
            Cli::Write { file, text } => {
                write(&self.main(), file, text);
                self.cli(jj, &["status"]);
            }
            Cli::New => {
                self.cli(jj, &["new"]);
            }
            Cli::Describe(message) => {
                self.cli(jj, &["describe", "-m", message]);
            }
            Cli::Bookmark(name) => {
                self.cli(
                    jj,
                    &["bookmark", "set", name, "-r", "@", "--allow-backwards"],
                );
            }
        }
    }
}

fn write(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// Each turn's operations are tau's exactly when tau took the turn, as
/// `is_taus` tells and as `jj op log` describes them.
#[hegel::test(
    test_cases = 25,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn the_jj_cli_and_tau_tell_whose_each_operation_is(tc: TestCase) {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!("no jj CLI of jj-lib's version: did not compare");
        return;
    };
    let turns: Vec<Turn> = tc.draw(gs::vecs(turn()).min_size(1).max_size(10));
    let mut world = World::new();
    let mut log = ops::OpLog::open(&world.main().join(".jj").join("repo"));
    // Whose each operation is, by id: what tau set up is tau's, but the
    // root, which is no one's.
    let mut taus: HashMap<String, bool> = HashMap::new();
    for op in log.new_ops() {
        let root = ops::is_root(&op);
        if root {
            assert!(!is_taus(&op).unwrap(), "the root is not tau's");
        } else {
            ops::assert_taus(&op, "setup");
        }
        taus.insert(op.id().hex(), !root);
    }
    for turn in &turns {
        match turn {
            Turn::Tau(step) => world.tau(step),
            Turn::Cli(step) => world.jj(&jj, step),
        }
        let by_tau = matches!(turn, Turn::Tau(_));
        for op in log.new_ops() {
            let what = format!("{turn:?}");
            if by_tau {
                ops::assert_taus(&op, &what);
            } else {
                assert!(
                    !is_taus(&op).unwrap(),
                    "{what}: the CLI's {:?} taken for tau's",
                    op.metadata().description
                );
            }
            taus.insert(op.id().hex(), by_tau);
        }
    }
    // The CLI's own log: every operation, tau's described `tau: …`, the
    // CLI's and the root not.
    let listed = world.cli(
        &jj,
        &[
            "op",
            "log",
            "--no-graph",
            "--ignore-working-copy",
            "-T",
            r#"id ++ "\t" ++ description.first_line() ++ "\n""#,
        ],
    );
    let mut seen = Vec::new();
    for line in listed.lines() {
        let (id, description) = line.split_once('\t').unwrap();
        let by_tau = taus[id];
        assert_eq!(
            description.starts_with(DESCRIPTION_PREFIX),
            by_tau,
            "jj op log: {line}"
        );
        seen.push(id);
    }
    assert_eq!(seen.len(), taus.len(), "jj op log lists every operation");
    tc.event_value("operations", taus.len() as f64);
}
