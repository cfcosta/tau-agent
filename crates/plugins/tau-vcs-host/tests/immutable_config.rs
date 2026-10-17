//! What is immutable is what the person's jj config says
//! (`docs/reference/vcs.md`, "Scoping rules"): the root, and the
//! ancestors of `immutable_heads()`, jj's default being `trunk()`, tags,
//! and untracked remote bookmarks, or the person's own definition, with
//! their aliases. Checked against a model of that rule on made-up
//! histories: the flags `vcs_log` shows, and whether a write tool
//! refuses `@`; where the jj CLI 0.45.1 is installed, `immutable()`
//! names the same commits there.
//!
//! Each world lives in a temporary directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    process::Command,
};

use hegel::{TestCase, generators as gs};
use jj_lib::{
    backend::CommitId,
    config::StackedConfig,
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    object_id::ObjectId as _,
    op_store::{RefTarget, RemoteRef, RemoteRefState},
    ref_name::{RefName, RemoteName},
    settings::UserSettings,
    workspace::Workspace,
};
use serde_json::json;
use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
use tau_testing::block_on_io as block_on;
use tau_vcs_host::{Identity, JjEnv, Vcs, VcsPlugin};

/// What marks a commit.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    hegel::DefaultGenerator,
    hegel::PrettyPrintable,
)]
enum Mark {
    Tag,
    /// A remote bookmark the repository tracks.
    Tracked,
    /// A remote bookmark it does not.
    Untracked,
    /// `main@origin`, which jj's `trunk()` names, tracked or not.
    Trunk {
        tracked: bool,
    },
    /// The local bookmark `frozen`.
    Frozen,
}

/// What the person's config says `immutable_heads()` is.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    hegel::DefaultGenerator,
    hegel::PrettyPrintable,
)]
enum Rule {
    /// jj's own.
    Default,
    /// jj's, and `frozen()`, an alias of the person's for the local
    /// bookmark `frozen`.
    PlusFrozen,
    /// Nothing: only the root.
    Nothing,
}

impl Rule {
    fn config(self) -> &'static str {
        match self {
            Self::Default => "",
            Self::PlusFrozen => {
                "[revset-aliases]\n'frozen()' = 'bookmarks(exact:\"frozen\")'\n\
                 'immutable_heads()' = 'builtin_immutable_heads() | frozen()'\n"
            }
            Self::Nothing => {
                "[revset-aliases]\n'immutable_heads()' = 'none()'\n"
            }
        }
    }
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    /// Commits in a line on the root, then `@` on the last.
    commits: usize,
    /// Each mark and the commit it is on: `commits` is `@`.
    marks: Vec<(usize, Mark)>,
    rule: Rule,
}

#[hegel::composite]
fn world(tc: &TestCase) -> World {
    let commits = tc.draw(gs::integers::<usize>().min_value(1).max_value(5));
    let marks = tc.draw(
        gs::vecs(hegel::compose!(|tc| {
            (
                tc.draw(gs::integers::<usize>().max_value(commits)),
                tc.draw(gs::default::<Mark>()),
            )
        }))
        .max_size(4),
    );
    World {
        commits,
        marks,
        rule: tc.draw(gs::default::<Rule>()),
    }
}

// The model: jj's rule, written out.

/// The commits, by place in the line (`commits` for `@`), that are
/// immutable heads in `world`. `main@origin` counts once, at its first
/// mark, as the world writes it; so does `frozen`.
fn heads(world: &World) -> BTreeSet<usize> {
    let mut heads = BTreeSet::new();
    let mut trunk = false;
    let mut frozen = false;
    for (at, mark) in &world.marks {
        let head = match mark {
            Mark::Tag | Mark::Untracked => true,
            Mark::Tracked => false,
            Mark::Trunk { .. } if trunk => false,
            // `trunk()` names it; untracked, it counts twice over.
            Mark::Trunk { .. } => {
                trunk = true;
                true
            }
            Mark::Frozen if frozen => false,
            Mark::Frozen => {
                frozen = true;
                world.rule == Rule::PlusFrozen
            }
        };
        if head && world.rule != Rule::Nothing {
            heads.insert(*at);
        }
    }
    heads
}

/// Whether the commit at `at` is immutable: an immutable head is it or
/// above it in the line.
fn immutable(world: &World, at: usize) -> bool {
    heads(world).iter().any(|head| *head >= at)
}

// The world on disk.

struct Disk {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Disk {
    fn new(world: &World) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let jj = root.join("config").join("jj");
        std::fs::create_dir_all(&jj).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::write(jj.join("config.toml"), world.rule.config()).unwrap();
        Self { _dir: dir, root }
    }

    fn work(&self) -> PathBuf {
        self.root.join("work")
    }

    fn identity(&self) -> Identity {
        Identity {
            env: JjEnv {
                config_dir: Some(self.root.join("config")),
                home_dir: Some(self.root.join("home")),
                ..JjEnv::default()
            },
            ..Identity::default()
        }
    }

    /// Makes the line of commits and marks them. Returns the tools on
    /// the workspace, and the commits' ids, `@`'s last.
    fn lay_out(&self, world: &World) -> (VcsPlugin, Vec<CommitId>) {
        let vcs = block_on(Vcs::init(self.work(), self.identity())).unwrap();
        let mut ids = Vec::new();
        for k in 0..world.commits {
            std::fs::write(self.work().join(format!("{k}.txt")), "x\n")
                .unwrap();
            let committed =
                block_on(vcs.commit_all(format!("c{k}"), "work")).unwrap();
            ids.push(CommitId::try_from_hex(&committed.commit_id).unwrap());
        }
        ids.push(working_copy(&self.work()));
        mark(&self.work(), world, &ids);
        (VcsPlugin::new(vcs), ids)
    }
}

/// Loads the workspace at `dir` with jj's defaults alone, as another
/// program would.
fn load(dir: &Path) -> Workspace {
    let settings = UserSettings::from_config({
        let mut config = StackedConfig::with_defaults();
        let mut user = jj_lib::config::ConfigLayer::empty(
            jj_lib::config::ConfigSource::User,
        );
        user.set_value("user.name", "host").unwrap();
        user.set_value("user.email", "host@localhost").unwrap();
        config.add_layer(user);
        config
    })
    .unwrap();
    Workspace::load(
        &settings,
        dir,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap()
}

fn working_copy(dir: &Path) -> CommitId {
    let workspace = load(dir);
    let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
    repo.view()
        .get_wc_commit_id(workspace.workspace_name())
        .unwrap()
        .clone()
}

/// Sets the world's marks with jj-lib, as the host or a fetch would.
fn mark(dir: &Path, world: &World, ids: &[CommitId]) {
    let workspace = load(dir);
    let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
    let mut tx = repo.start_transaction();
    let mut trunk = false;
    let mut frozen = false;
    let origin = RemoteName::new("origin");
    for (n, (at, mark)) in world.marks.iter().enumerate() {
        let target = RefTarget::normal(ids[*at].clone());
        let remote = |state| RemoteRef {
            target: target.clone(),
            state,
        };
        match mark {
            Mark::Tag => tx
                .repo_mut()
                .set_local_tag_target(RefName::new(&format!("t{n}")), target),
            Mark::Tracked | Mark::Untracked => {
                let state = if *mark == Mark::Tracked {
                    RemoteRefState::Tracked
                } else {
                    RemoteRefState::New
                };
                let name = format!("b{n}");
                tx.repo_mut().set_remote_bookmark(
                    RefName::new(&name).to_remote_symbol(origin),
                    remote(state),
                );
            }
            Mark::Trunk { tracked } if !trunk => {
                trunk = true;
                let state = if *tracked {
                    RemoteRefState::Tracked
                } else {
                    RemoteRefState::New
                };
                tx.repo_mut().set_remote_bookmark(
                    RefName::new("main").to_remote_symbol(origin),
                    remote(state),
                );
            }
            Mark::Frozen if !frozen => {
                frozen = true;
                tx.repo_mut()
                    .set_local_bookmark_target(RefName::new("frozen"), target);
            }
            Mark::Trunk { .. } | Mark::Frozen => {}
        }
    }
    block_on(tx.commit("mark")).unwrap();
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

/// Each commit's immutable flag, by commit id, as `vcs_log` shows it.
fn flags(tools: &VcsPlugin) -> HashMap<String, bool> {
    let log = call(tools, "vcs_log", json!({ "limit": 100 })).unwrap();
    log["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|change| {
            (
                change["commit_id"].as_str().unwrap().to_owned(),
                change["immutable"].as_bool().unwrap(),
            )
        })
        .collect()
}

/// `vcs_log`'s immutable flags, and a write tool's refusal of `@`, are
/// the model's.
#[hegel::test(test_cases = 100)]
fn immutability_is_what_the_config_says(tc: TestCase) {
    let world = tc.draw(world());
    let disk = Disk::new(&world);
    let (tools, ids) = disk.lay_out(&world);
    let flags = flags(&tools);
    for (at, id) in ids.iter().enumerate() {
        assert_eq!(
            flags.get(&id.hex()).copied(),
            Some(immutable(&world, at)),
            "commit {at} of {}",
            ids.len()
        );
    }
    let described = call(&tools, "vcs_describe", json!({ "message": "edit" }));
    let at = world.commits;
    match described {
        Err(error) => {
            assert!(immutable(&world, at), "{error}");
            assert!(error.ends_with(" is immutable"), "{error}");
        }
        Ok(_) => assert!(!immutable(&world, at)),
    }
}

/// A broken `immutable_heads()` is an error naming it, for every tool,
/// never a fallback to jj's default.
#[test]
fn a_broken_immutable_heads_is_an_error_naming_it() {
    for definition in ["'nosuch()'", "'trunk('", "'no_such_bookmark'"] {
        let disk = Disk::new(&World {
            commits: 1,
            marks: Vec::new(),
            rule: Rule::Default,
        });
        std::fs::write(
            disk.root.join("config/jj/config.toml"),
            format!("[revset-aliases]\n'immutable_heads()' = {definition}\n"),
        )
        .unwrap();
        let vcs = block_on(Vcs::init(disk.work(), disk.identity())).unwrap();
        let tools = VcsPlugin::new(vcs);
        for name in ["vcs_status", "vcs_log"] {
            let error = call(&tools, name, json!({})).unwrap_err();
            assert!(
                error.contains("revset-aliases.immutable_heads()"),
                "{definition} {name}: {error}"
            );
        }
    }
}

/// Where the jj CLI of jj-lib's own version is installed, its
/// `immutable()` is tau's, and the model's, in the same world. Elsewhere
/// this test says it did not run.
#[hegel::test(test_cases = 40)]
fn the_jj_cli_finds_the_same_immutable_commits(tc: TestCase) {
    let Some(jj) = matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let world = tc.draw(world());
    let disk = Disk::new(&world);
    let (tools, ids) = disk.lay_out(&world);
    let ours: BTreeSet<String> = flags(&tools)
        .into_iter()
        .filter(|(id, immutable)| *immutable && *id != "0".repeat(40))
        .map(|(id, _)| id)
        .collect();
    let output = Command::new(&jj)
        .env_clear()
        .env("HOME", disk.root.join("home"))
        .env("XDG_CONFIG_HOME", disk.root.join("config"))
        .current_dir(disk.work())
        .args([
            "log",
            "--ignore-working-copy",
            "--no-graph",
            "-r",
            "immutable() ~ root()",
            "-T",
            "commit_id ++ \"\\n\"",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let theirs: BTreeSet<String> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    let model: BTreeSet<String> = ids
        .iter()
        .enumerate()
        .filter(|(at, _)| immutable(&world, *at))
        .map(|(_, id)| id.hex())
        .collect();
    assert_eq!(theirs, ours);
    assert_eq!(ours, model);
}

/// The installed jj CLI, if its version is jj-lib's, by its full path:
/// the CLI runs with no `PATH`.
fn matching_jj() -> Option<PathBuf> {
    let jj = std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("jj"))
        .find(|path| path.is_file())?;
    let output = Command::new(&jj).arg("--version").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?;
    (version.trim() == "jj 0.45.1").then_some(jj)
}
