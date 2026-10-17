//! tau follows the person's jj config as the jj CLI does
//! (`docs/reference/vcs.md`, "Identity and jj config"): the user's
//! files, `JJ_CONFIG`, the repository's and the workspace's configs,
//! `--when` scopes and the variables that override them, with tau's
//! identity only where none names the author. Each test makes up the
//! person's environment in a temporary directory, and reads no real one.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use hegel::{TestCase, generators as gs};
use jj_lib::secure_config::SecureConfig;
use rand_chacha::{ChaCha20Rng, rand_core::SeedableRng as _};
use serde_json::json;
use tau_agent::{plugin::Plugin, tool::ToolCtx};
use tau_testing::{block_on_io, git::git};
use tau_vcs_host::{Identity, JjEnv, ProjectRepo, Vcs, VcsPlugin};

/// A made-up person's environment: a home and a config directory,
/// canonical, as jj sees them.
struct Person {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Person {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        Self { _dir: dir, root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    /// jj's own config directory.
    fn jj_dir(&self) -> PathBuf {
        self.root.join("config").join("jj")
    }

    fn env(&self, vars: &[(&str, String)]) -> JjEnv {
        JjEnv {
            config_dir: Some(self.root.join("config")),
            home_dir: Some(self.home()),
            system_dir: Some(self.root.join("etc")),
            hostname: Some("desk".into()),
            username: Some("person".into()),
            vars: vars
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect::<HashMap<_, _>>(),
        }
    }

    fn identity(&self, vars: &[(&str, String)]) -> Identity {
        Identity {
            env: self.env(vars),
            ..Identity::default()
        }
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A TOML basic string: JSON's escapes are TOML's too.
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap()
}

fn user(name: &str, email: &str) -> String {
    format!(
        "user.name = {}\nuser.email = {}\n",
        quoted(name),
        quoted(email)
    )
}

/// Commits a file in `vcs` twice and returns the second commit's
/// author, as `vcs_show` shows it. The first commits `@` as it was, and
/// a change keeps the author it was made with, as in jj; the second is
/// a change made as `vcs` is now.
fn author_of_a_commit(vcs: &Vcs) -> (String, String) {
    let mut committed = None;
    for _ in 0..2 {
        std::fs::write(
            vcs.root().join("a.txt"),
            uuid::Uuid::now_v7().to_string(),
        )
        .unwrap();
        committed =
            Some(block_on_io(vcs.commit_all("a change", "work")).unwrap());
    }
    let committed = committed.unwrap();
    let show = VcsPlugin::new(vcs.clone())
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_show")
        .unwrap();
    let details = block_on_io(show.call(
        json!({ "change": committed.commit_id }),
        ToolCtx::detached(),
    ))
    .unwrap()
    .details
    .unwrap();
    let author = &details["author"];
    (
        author["name"].as_str().unwrap().to_owned(),
        author["email"].as_str().unwrap().to_owned(),
    )
}

/// The author of a commit in a new repository at `dir`.
fn author_in_new_repo(dir: &Path, identity: Identity) -> (String, String) {
    let vcs = block_on_io(Vcs::init(dir, identity)).unwrap();
    author_of_a_commit(&vcs)
}

fn names() -> impl hegel::generators::PrintableGenerator<String> {
    gs::from_regex("[A-Za-z]([A-Za-z .'-]{0,12}[A-Za-z])?").fullmatch(true)
}

fn emails() -> impl hegel::generators::PrintableGenerator<String> {
    gs::from_regex("[a-z0-9]{1,8}@[a-z]{1,8}\\.[a-z]{2,3}").fullmatch(true)
}

/// With no config naming anyone, a commit is by tau's identity,
/// whatever it is, and whether or not the person has directories.
#[hegel::test(test_cases = 10)]
fn with_no_config_a_commit_is_by_taus_identity(tc: TestCase) {
    let person = Person::new();
    let name = tc.draw(names());
    let email = tc.draw(emails());
    let env = if tc.draw(gs::booleans()) {
        person.env(&[])
    } else {
        JjEnv::default()
    };
    let identity = Identity {
        name: name.clone(),
        email: email.clone(),
        env,
    };
    let author = author_in_new_repo(&person.root.join("repo"), identity);
    assert_eq!(author, (name, email));
}

/// The person's `user.name` and `user.email` name the author, from
/// `~/.jjconfig.toml` or jj's `config.toml`; tau's identity does not.
#[hegel::test(test_cases = 10)]
fn the_persons_config_names_the_author(tc: TestCase) {
    let person = Person::new();
    let name = tc.draw(names());
    let email = tc.draw(emails());
    let file = if tc.draw(gs::booleans()) {
        person.home().join(".jjconfig.toml")
    } else {
        person.jj_dir().join("config.toml")
    };
    write(&file, &user(&name, &email));
    let author =
        author_in_new_repo(&person.root.join("repo"), person.identity(&[]));
    assert_eq!(author, (name, email));
}

/// `conf.d`'s files load after `config.toml`, in name order, so the
/// last one names the author. `JJ_CONFIG`, when set, replaces every
/// default path, the system's too: a directory it names loads the same
/// way.
#[hegel::test(test_cases = 10)]
fn conf_d_loads_in_name_order_and_jj_config_replaces_the_defaults(
    tc: TestCase,
) {
    let person = Person::new();
    let stems: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-z0-9]{1,6}").fullmatch(true))
            .min_size(1)
            .max_size(5)
            .unique(true),
    );
    let via_jj_config = tc.draw(gs::booleans());
    let dir = if via_jj_config {
        person.root.join("elsewhere")
    } else {
        person.jj_dir().join("conf.d")
    };
    for stem in &stems {
        write(
            &dir.join(format!("{stem}.toml")),
            &user(stem, "d@example.com"),
        );
    }
    // Neither of these is read under `JJ_CONFIG`.
    write(
        &person.jj_dir().join("config.toml"),
        &user("Platform", "platform@example.com"),
    );
    write(
        &person.root.join("etc/jj/config.toml"),
        "user.email = \"system@example.com\"\n",
    );
    let vars = if via_jj_config {
        let file = person.root.join("first.toml");
        write(&file, "user.email = \"first@example.com\"\n");
        let paths = std::env::join_paths([file, dir.clone()]).unwrap();
        vec![("JJ_CONFIG", paths.into_string().unwrap())]
    } else {
        // A file of another kind in conf.d is not config.
        write(&dir.join("zzzz.txt"), "user.name = \"Not config\"\n");
        Vec::new()
    };
    let author =
        author_in_new_repo(&person.root.join("repo"), person.identity(&vars));
    let last = stems.iter().max().unwrap().clone();
    assert_eq!(author, (last, "d@example.com".to_owned()));
}

/// The repository's config, which jj keeps in the person's config
/// directory under the repository's config id, overrides the person's;
/// the workspace's overrides both. A repository with no config id gets
/// none from tau.
#[test]
fn the_repository_and_workspace_configs_override_the_persons() {
    let person = Person::new();
    write(
        &person.jj_dir().join("config.toml"),
        &user("Person", "person@example.com"),
    );
    let identity = person.identity(&[]);
    let repo = person.root.join("repo");
    block_on_io(Vcs::init(&repo, identity.clone())).unwrap();
    let mut rng = ChaCha20Rng::seed_from_u64(0);
    let repo_path = std::fs::canonicalize(repo.join(".jj/repo")).unwrap();
    let repo_config = SecureConfig::new_repo(repo_path)
        .load_config(&mut rng, &person.jj_dir().join("repos"))
        .unwrap()
        .config_file
        .unwrap();
    write(&repo_config, &user("Repository", "repository@example.com"));
    let workspace_config = SecureConfig::new_workspace(repo.join(".jj"))
        .load_config(&mut rng, &person.jj_dir().join("workspaces"))
        .unwrap()
        .config_file
        .unwrap();
    write(
        &workspace_config,
        "user.email = \"workspace@example.com\"\n",
    );

    let vcs = block_on_io(Vcs::open(&repo, identity.clone())).unwrap();
    assert_eq!(
        author_of_a_commit(&vcs),
        ("Repository".to_owned(), "workspace@example.com".to_owned())
    );

    let other = person.root.join("other");
    assert_eq!(
        author_in_new_repo(&other, identity),
        ("Person".to_owned(), "person@example.com".to_owned())
    );
    assert!(!other.join(".jj/repo/config-id").exists());
    assert!(!other.join(".jj/workspace-config-id").exists());
    let ids = std::fs::read_dir(person.jj_dir().join("repos")).unwrap();
    assert_eq!(ids.count(), 1);
}

/// A `--when.repositories` scope holds in its repository only, with `~`
/// for the person's home.
#[test]
fn a_repository_scope_applies_only_to_its_repository() {
    let person = Person::new();
    write(
        &person.jj_dir().join("config.toml"),
        &format!(
            "{}\n[[--scope]]\n--when.repositories = [\"~/scoped\"]\n\
             user.name = \"Scoped\"\n",
            user("Everywhere", "person@example.com")
        ),
    );
    let identity = person.identity(&[]);
    assert_eq!(
        author_in_new_repo(&person.home().join("scoped"), identity.clone()),
        ("Scoped".to_owned(), "person@example.com".to_owned())
    );
    assert_eq!(
        author_in_new_repo(&person.home().join("other"), identity),
        ("Everywhere".to_owned(), "person@example.com".to_owned())
    );
}

/// `JJ_USER` and `JJ_EMAIL` override every config file, the
/// repository's too.
#[hegel::test(test_cases = 10)]
fn jj_user_and_jj_email_override_the_config(tc: TestCase) {
    let person = Person::new();
    let name = tc.draw(names());
    let email = tc.draw(emails());
    write(
        &person.jj_dir().join("config.toml"),
        &user("Person", "person@example.com"),
    );
    let vars = [("JJ_USER", name.clone()), ("JJ_EMAIL", email.clone())];
    let author =
        author_in_new_repo(&person.root.join("repo"), person.identity(&vars));
    assert_eq!(author, (name, email));
}

/// A project's run workspace, which tau makes from the main one, commits
/// as the person too.
#[test]
fn a_projects_run_workspace_commits_as_the_person() {
    let person = Person::new();
    write(
        &person.jj_dir().join("config.toml"),
        &user("Person", "person@example.com"),
    );
    let src = person.root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("a.txt"), "one\n").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    let project = ProjectRepo::import(
        src.to_str().unwrap(),
        person.root.join("project"),
        person.identity(&[]),
    )
    .unwrap();
    let vcs = project
        .add_workspace("run", &project.trunk().unwrap())
        .unwrap();
    assert_eq!(
        author_of_a_commit(&vcs),
        ("Person".to_owned(), "person@example.com".to_owned())
    );
}
