//! A clone sets the repository's `trunk()` to the remote's default
//! branch as `jj git clone` does (`docs/reference/vcs.md`, "Projects"),
//! so `immutable_heads()` names upstream's history whatever the default
//! branch is called, and a `trunk()` that names nothing the repository
//! has falls back to the root, as jj's does:
//!
//! - when `origin/HEAD` names a branch the clone has, every commit of
//!   that branch is immutable, and the repository's config holds
//!   `revset-aliases."trunk()" = "<branch>@origin"`, with what the file
//!   held before kept;
//! - where the jj CLI of jj-lib's version is found, `jj git clone` of
//!   the same remote leaves the same repository config, byte for byte,
//!   and `jj log -r 'immutable()'` in tau's clone names the same
//!   commits tau does.
//!
//! Each world lives in a temporary directory and reads no real config.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

use hegel::{TestCase, generators as gs};
use jj_lib::{
    config::{ConfigLayer, ConfigSource},
    ref_name::{RefName, RemoteName},
    secure_config::SecureConfig,
};
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use serde_json::json;
use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
use tau_testing::{block_on_io as block_on, git::git};
use tau_vcs_host::{Identity, JjEnv, ProjectRepo, Vcs, VcsPlugin};

/// What both jj and tau seed their config ids with here, so a config
/// written before the clone is at the path the clone's would be.
const SEED: u64 = 7;

/// Branch names jj's `trunk()` knows, and others, some of which a
/// revset must quote.
const NAMES: [&str; 8] = [
    "main",
    "master",
    "trunk",
    "develop",
    "side",
    "release/1.x",
    "feat-a",
    "v2.0",
];

/// What the repository's config held before the clone, at the path
/// its config gets.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Existing {
    editor: Option<String>,
    /// A `trunk()` of its own, naming this branch on `origin`.
    trunk: Option<String>,
    /// Another alias, which the write must keep.
    alias: bool,
}

#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct World {
    /// The remote's default branch.
    default: String,
    /// Whether the remote's `HEAD` names it, or a branch it lacks.
    head_names_it: bool,
    /// Other branches, each one commit off the default branch's first.
    others: Vec<String>,
    /// The person's own `trunk()`, naming this branch on `origin`.
    user_trunk: Option<String>,
    existing: Option<Existing>,
}

#[hegel::composite]
fn branch(tc: &TestCase) -> String {
    if tc.draw(gs::booleans()) {
        tc.draw(gs::sampled_from(NAMES.to_vec())).to_owned()
    } else {
        tc.draw(
            gs::from_regex(
                "[a-z][a-z0-9]{0,5}(/[a-z][a-z0-9]{0,4})?(-[a-z0-9]{1,3})?",
            )
            .fullmatch(true),
        )
    }
}

#[hegel::composite]
fn world(tc: &TestCase) -> World {
    let default = tc.draw(branch());
    let others: Vec<&str> = tc.draw(
        gs::vecs(gs::sampled_from(NAMES.to_vec()))
            .max_size(3)
            .unique(true),
    );
    let others = others
        .into_iter()
        .filter(|name| *name != default)
        .map(str::to_owned)
        .collect();
    let named = || {
        hegel::compose!(|tc| {
            if tc.draw(gs::booleans()) {
                tc.draw(gs::sampled_from(NAMES.to_vec())).to_owned()
            } else {
                // Nothing the remote has.
                "nosuch".to_owned()
            }
        })
    };
    let existing = tc.draw(gs::optional(hegel::compose!(|tc| {
        Existing {
            editor: tc.draw(gs::optional(
                gs::from_regex("[a-z]{1,8}").fullmatch(true),
            )),
            trunk: tc.draw(gs::optional(named())),
            alias: tc.draw(gs::booleans()),
        }
    })));
    World {
        default,
        head_names_it: tc.draw(gs::booleans()),
        others,
        user_trunk: tc.draw(gs::optional(named())),
        existing,
    }
}

/// A `trunk()` alias naming `branch` on `origin`, quoted for jj.
fn trunk_line(branch: &str) -> String {
    format!("'trunk()' = '\"{branch}\"@origin'\n")
}

impl Existing {
    fn text(&self) -> String {
        let mut text = String::new();
        if let Some(editor) = &self.editor {
            text.push_str(&format!("[ui]\neditor = \"{editor}\"\n\n"));
        }
        if self.trunk.is_some() || self.alias {
            text.push_str("[revset-aliases]\n");
        }
        if let Some(branch) = &self.trunk {
            text.push_str(&trunk_line(branch));
        }
        if self.alias {
            text.push_str("'old()' = 'none()'\n");
        }
        text
    }
}

/// The remote, as a bare repository behind a `file://` URL. Returns the
/// URL and each branch's commits, oldest first.
fn upstream(
    root: &Path,
    world: &World,
) -> (String, Vec<(String, Vec<String>)>) {
    let bare = root.join("origin.git");
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "--quiet", "--bare"]);
    let work = root.join("upstream-work");
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "--quiet"]);
    git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
    let mut branches = Vec::new();
    let mut commits = Vec::new();
    for k in 0..2 {
        git(
            &work,
            &["commit", "--quiet", "--allow-empty", "-m", &format!("d{k}")],
        );
        commits.push(git(&work, &["rev-parse", "HEAD"]));
    }
    let refspec = format!("HEAD:refs/heads/{}", world.default);
    git(&work, &["push", "--quiet", "origin", &refspec]);
    branches.push((world.default.clone(), commits.clone()));
    for other in &world.others {
        git(&work, &["checkout", "--quiet", "--detach", &commits[0]]);
        git(
            &work,
            &["commit", "--quiet", "--allow-empty", "-m", other.as_str()],
        );
        let id = git(&work, &["rev-parse", "HEAD"]);
        let refspec = format!("HEAD:refs/heads/{other}");
        git(&work, &["push", "--quiet", "origin", &refspec]);
        branches.push((other.clone(), vec![commits[0].clone(), id]));
    }
    let head = if world.head_names_it {
        format!("refs/heads/{}", world.default)
    } else {
        "refs/heads/gone".to_owned()
    };
    git(&bare, &["symbolic-ref", "HEAD", &head]);
    (format!("file://{}", bare.display()), branches)
}

/// A person: their home and config directory, with `world`'s config,
/// and the repository config `world` says was there before.
struct Person {
    root: PathBuf,
}

impl Person {
    fn new(root: PathBuf, world: &World) -> Self {
        let jj = root.join("config").join("jj");
        std::fs::create_dir_all(&jj).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        let mut config =
            "user.name = \"Person\"\nuser.email = \"p@example.com\"\n"
                .to_owned();
        if let Some(branch) = &world.user_trunk {
            config.push_str("[revset-aliases]\n");
            config.push_str(&trunk_line(branch));
        }
        std::fs::write(jj.join("config.toml"), config).unwrap();
        if let Some(existing) = &world.existing {
            // The config jj made for a repository that was here before,
            // and is gone: a new one with the same seed gets its id.
            let gone = root.join("gone");
            std::fs::create_dir_all(&gone).unwrap();
            let loaded = SecureConfig::new_repo(gone.clone())
                .load_config(
                    &mut ChaCha20Rng::seed_from_u64(SEED),
                    &jj.join("repos"),
                )
                .unwrap();
            std::fs::write(loaded.config_file.unwrap(), existing.text())
                .unwrap();
            std::fs::remove_dir_all(gone).unwrap();
        }
        Self { root }
    }

    fn identity(&self) -> Identity {
        Identity {
            env: JjEnv {
                config_dir: Some(self.root.join("config")),
                home_dir: Some(self.root.join("home")),
                vars: [("JJ_RANDOMNESS_SEED".to_owned(), SEED.to_string())]
                    .into(),
                ..JjEnv::default()
            },
            ..Identity::default()
        }
    }

    /// The config of the repository at `repo` (its `.jj/repo`), as a
    /// file: its text, or none when the repository has no config.
    fn repo_config(&self, repo: &Path) -> Option<String> {
        let id = std::fs::read_to_string(repo.join("config-id")).ok()?;
        let path = self
            .root
            .join("config/jj/repos")
            .join(id.trim())
            .join("config.toml");
        std::fs::read_to_string(path).ok()
    }

    /// The jj CLI as this person runs it: no other variable of this
    /// process but `PATH`, for `git`.
    fn cli(&self, jj: &Path) -> Command {
        let mut command = Command::new(jj);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("JJ_RANDOMNESS_SEED", SEED.to_string());
        command
    }
}

fn show(tools: &VcsPlugin, commit: &str) -> bool {
    let tool = tools
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_show")
        .unwrap();
    let output =
        block_on(tool.call(json!({ "change": commit }), ToolCtx::detached()))
            .unwrap_or_else(|error| panic!("vcs_show {commit}: {error}"));
    output.details.unwrap()["change"]["immutable"]
        .as_bool()
        .unwrap()
}

/// What tau's clone says of any world, and what `jj git clone` and `jj
/// log` say of the same remote and clone, where that jj is found.
#[hegel::test(test_cases = 40)]
fn a_clone_sets_trunk_as_jj_git_clone_does(tc: TestCase) {
    check(&tc.draw(world()));
}

/// What a repository's config held before its clone is still there
/// after, beside the `trunk()` the clone wrote over any it had.
#[hegel::test(test_cases = 20)]
fn a_clone_keeps_what_the_repository_config_held(tc: TestCase) {
    let world = tc.draw(world());
    let existing = Existing {
        editor: Some(tc.draw(gs::from_regex("[a-z]{1,8}").fullmatch(true))),
        ..world.existing.clone().unwrap_or(Existing {
            editor: None,
            trunk: None,
            alias: true,
        })
    };
    check(&World {
        head_names_it: true,
        // So `jj git clone` clones too, for the comparison.
        user_trunk: None,
        existing: Some(existing),
        ..world
    });
}

/// Clones `world`'s remote with tau, checks it against the model, and
/// against the jj CLI where it is found.
fn check(world: &World) {
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let (url, branches) = upstream(&root, world);
    let person = Person::new(root.join("tau"), world);
    let project =
        ProjectRepo::clone(&url, None, root.join("p"), person.identity())
            .unwrap();
    let main = project.main_dir().to_owned();
    let repo = main.join(".jj").join("repo");
    let tools = VcsPlugin::new(
        block_on(Vcs::open(main.clone(), person.identity())).unwrap(),
    );
    let mut immutable = BTreeSet::new();
    let mut all = BTreeSet::new();
    for (_, commits) in &branches {
        for commit in commits {
            if all.insert(commit.clone()) && show(&tools, commit) {
                immutable.insert(commit.clone());
            }
        }
    }

    // The model: jj's rule for the default branch, and its write.
    let written = person.repo_config(&repo);
    if world.head_names_it {
        for commit in &branches[0].1 {
            assert!(
                immutable.contains(commit),
                "{commit} of the default branch"
            );
        }
        let written = written.as_deref().expect("the repository has a config");
        let config = ConfigLayer::parse(ConfigSource::Repo, written).unwrap();
        let get = |name: &[&str]| {
            config
                .look_up_item(name)
                .unwrap()
                .map(|item| item.as_str().unwrap().to_owned())
        };
        let symbol = RefName::new(&world.default)
            .to_remote_symbol(RemoteName::new("origin"))
            .to_string();
        assert_eq!(get(&["revset-aliases", "trunk()"]), Some(symbol));
        if let Some(existing) = &world.existing {
            assert_eq!(get(&["ui", "editor"]), existing.editor);
            assert_eq!(
                get(&["revset-aliases", "old()"]).is_some(),
                existing.alias
            );
        }
    } else {
        assert_eq!(written, None, "nothing to write, so no config is made");
    }

    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!(
            "the jj CLI is not installed at jj-lib's version: not compared"
        );
        return;
    };
    let theirs = Person::new(root.join("jj"), world);
    let clone = theirs
        .cli(&jj)
        .args(["git", "clone", "--colocate", "--quiet", &url])
        .arg(root.join("jj-clone"))
        .output()
        .unwrap();
    // `jj git clone` resolves `immutable_heads()` while the new
    // repository is still empty, without the fallback to `root()` every
    // other command has, so a `trunk()` of the person's naming a remote
    // bookmark refuses the clone. tau clones; what it then reads is
    // compared below.
    if world.user_trunk.is_some() {
        assert!(!clone.status.success(), "{clone:?}");
        let stderr = String::from_utf8_lossy(&clone.stderr);
        assert!(
            stderr.contains("Invalid `revset-aliases.immutable_heads()`"),
            "{stderr}"
        );
    } else {
        assert!(clone.status.success(), "{clone:?}");
        assert_eq!(
            written,
            theirs.repo_config(&root.join("jj-clone/.jj/repo")),
            "the repository config jj git clone writes"
        );
    }
    let log = person
        .cli(&jj)
        .current_dir(&main)
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
    assert!(log.status.success(), "{log:?}");
    let theirs: BTreeSet<String> = String::from_utf8(log.stdout)
        .unwrap()
        .lines()
        .filter(|id| all.contains(*id))
        .map(str::to_owned)
        .collect();
    assert_eq!(theirs, immutable);
}
