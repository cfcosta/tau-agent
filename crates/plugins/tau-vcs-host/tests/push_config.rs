//! A push follows the person's jj config as `jj git push` does
//! (`docs/reference/vcs.md`, "Pushing"), against a bare repository
//! reached through a `file://` URL:
//!
//! - a commit `git.private-commits` names, among those a push would send,
//!   refuses the push with an error naming it, the newest first, before
//!   anything is sent; a pull request's replay refuses it the same way;
//! - with `git.sign-on-push`, every commit sent that the person wrote
//!   arrives signed, and no other, by jj-lib's `test` signing backend;
//!   without it, commits arrive as they are;
//! - `git.executable-path` is the `git` a push runs;
//! - an `at_operation()` in `git.private-commits` sees the commits of an
//!   operation off the repository's line, as jj 0.46 evaluates it.
//!
//! The expectations are read from the remote with `git`: what arrived,
//! by whom, signed or not. Each person's config lives in a temporary
//! directory, and no real jj config is read.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::path::{Path, PathBuf};

use hegel::{TestCase, generators as gs};
use tau_testing::{block_on_io as block_on, git::git};
use tau_vcs_host::{
    DEFAULT_WORKSPACE,
    Identity,
    JjEnv,
    ProjectRepo,
    Remote,
    Vcs,
    VcsError,
};

const PERSON: &str = "person@example.com";
const OTHER: &str = "other@example.com";
/// What the test signing backend writes into a signed commit.
const SIGNATURE: &str = "JJ-TEST-SIGNATURE";

/// A person with a jj config, a bare repository standing for GitHub
/// whose `main` has one commit, and a project cloned from it.
struct Origin {
    _home: tempfile::TempDir,
    root: PathBuf,
    bare: PathBuf,
    url: String,
    project: ProjectRepo,
}

impl Origin {
    fn new(config: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(home.path()).unwrap();
        let jj = root.join("config").join("jj");
        std::fs::create_dir_all(&jj).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::write(
            jj.join("config.toml"),
            format!(
                "user.name = \"Person\"\nuser.email = \"{PERSON}\"\n\
                 signing.backend = \"test\"\n{config}"
            ),
        )
        .unwrap();
        let bare = root.join("origin.git");
        std::fs::create_dir_all(&bare).unwrap();
        git(&bare, &["init", "--quiet", "--bare"]);
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--quiet"]);
        git(
            &work,
            &["commit", "--quiet", "--allow-empty", "-m", "first"],
        );
        git(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&work, &["push", "--quiet", "origin", "HEAD:main"]);
        let url = format!("file://{}", bare.display());
        let identity = Self::identity_in(&root, PERSON);
        let project =
            ProjectRepo::clone(&url, None, root.join("p"), identity).unwrap();
        Self {
            _home: home,
            root,
            bare,
            url,
            project,
        }
    }

    fn identity_in(root: &std::path::Path, email: &str) -> Identity {
        Identity {
            env: JjEnv {
                config_dir: Some(root.join("config")),
                home_dir: Some(root.join("home")),
                vars: [("JJ_EMAIL".to_owned(), email.to_owned())].into(),
                ..JjEnv::default()
            },
            ..Identity::default()
        }
    }

    /// The workspace at `dir`, writing as `email`.
    fn vcs_as(&self, dir: PathBuf, email: &str) -> Vcs {
        block_on(Vcs::open(dir, Self::identity_in(&self.root, email))).unwrap()
    }

    fn remote(&self) -> Remote<'_> {
        Remote {
            url: &self.url,
            token: None,
        }
    }

    fn origin_ref(&self, name: &str) -> Option<String> {
        let output = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "--quiet", name])
            .current_dir(&self.bare)
            .output()
            .unwrap();
        output.status.success().then(|| {
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        })
    }

    /// The commits `range` names on the remote, oldest first: subject,
    /// author's email, and whether it is signed.
    fn arrived(&self, range: &str) -> Vec<(String, String, bool)> {
        let ids = git(&self.bare, &["rev-list", "--reverse", range]);
        ids.lines()
            .map(|id| {
                let subject =
                    git(&self.bare, &["log", "-1", "--format=%s", id]);
                let email = git(&self.bare, &["log", "-1", "--format=%ae", id]);
                let raw = git(&self.bare, &["cat-file", "commit", id]);
                (subject, email, raw.contains(SIGNATURE))
            })
            .collect()
    }
}

/// One commit a world makes: by the person or someone else, private or
/// not.
#[derive(Debug, Clone, hegel::PrettyPrintable)]
struct Made {
    theirs: bool,
    private: bool,
}

#[hegel::composite]
fn made(tc: &TestCase) -> Vec<Made> {
    tc.draw(
        gs::vecs(hegel::compose!(|tc| {
            Made {
                theirs: tc.draw(gs::booleans()),
                // A quarter of them, so most stacks push.
                private: tc.draw(gs::integers::<u8>().max_value(3)) == 0,
            }
        }))
        .min_size(1)
        .max_size(4),
    )
}

fn subject(k: usize, made: &Made) -> String {
    if made.private {
        format!("private {k}")
    } else {
        format!("work {k}")
    }
}

const PRIVATE: &str =
    "git.private-commits = 'description(glob:\"private*\")'\n";

/// Makes `commits` in the workspace at `dir`, each a file of its own.
/// A change keeps the author its `@` was made with, so the handle that
/// commits one makes the next one's `@`: commit `k` is by whoever
/// `commits[k]` names, the first by the person, whose `@` it was.
fn commit_all(
    origin: &Origin,
    dir: &std::path::Path,
    bookmark: &str,
    prefix: &str,
    commits: &[Made],
) -> Vec<String> {
    let mut ids = Vec::new();
    for (k, made) in commits.iter().enumerate() {
        let next = commits.get(k + 1).is_some_and(|next| next.theirs);
        let vcs =
            origin.vcs_as(dir.to_owned(), if next { OTHER } else { PERSON });
        std::fs::write(dir.join(format!("{prefix}{k}.txt")), "x\n").unwrap();
        let committed =
            block_on(vcs.commit_all(subject(k, made), bookmark)).unwrap();
        ids.push(committed.commit_id);
    }
    ids
}

/// The person's own commits, where `theirs` says who: the first is
/// always the person's (see [`commit_all`]).
fn authors(commits: &mut [Made]) {
    if let Some(first) = commits.first_mut() {
        first.theirs = false;
    }
}

/// Trunk's push: refused, naming the newest private commit, with
/// nothing sent; otherwise each commit arrives, signed exactly when
/// `git.sign-on-push` is set and the person wrote it.
#[hegel::test(test_cases = 40)]
fn a_trunk_push_refuses_private_commits_and_signs_the_persons(tc: TestCase) {
    let mut commits = tc.draw(made());
    authors(&mut commits);
    let sign = tc.draw(gs::booleans());
    let origin = Origin::new(&format!("{PRIVATE}git.sign-on-push = {sign}\n"));
    let main = origin.project.workspace_dir(DEFAULT_WORKSPACE);
    let trunk = origin.project.trunk_name().unwrap();
    let before = origin.origin_ref("main");
    let ids = commit_all(&origin, &main, &trunk, "m", &commits);
    let local = origin.project.trunk().unwrap();
    let result = origin.project.push_trunk(origin.remote());
    match commits.iter().rposition(|made| made.private) {
        Some(newest) => {
            let error = result.unwrap_err();
            assert!(matches!(error, VcsError::PrivateCommit { .. }), "{error}");
            let text = error.to_string();
            assert!(
                text.contains(&subject(newest, &commits[newest])),
                "{text}"
            );
            assert!(text.contains(&ids[newest][..12]), "{text}");
            assert_eq!(origin.origin_ref("main"), before, "nothing sent");
            assert_eq!(
                origin.project.trunk().unwrap(),
                local,
                "nothing signed"
            );
        }
        None => {
            let pushed = result.unwrap();
            let expected: Vec<(String, String, bool)> = commits
                .iter()
                .enumerate()
                .map(|(k, made)| {
                    let email = if made.theirs { OTHER } else { PERSON };
                    (subject(k, made), email.to_owned(), sign && !made.theirs)
                })
                .collect();
            let before = before.clone().unwrap();
            assert_eq!(origin.arrived(&format!("{before}..main")), expected);
            assert_eq!(origin.origin_ref("main"), Some(pushed.to.clone()));
            assert_eq!(origin.project.trunk().unwrap(), pushed.to);
            if !sign {
                assert_eq!(pushed.to, local, "nothing rewritten");
            }
        }
    }
}

/// A pull request's replay refuses a private commit before any copy is
/// made; its copies arrive signed exactly when `git.sign-on-push` is set
/// and the person wrote them, and a later push signs only its own.
#[hegel::test(test_cases = 40)]
fn a_branch_push_refuses_private_commits_and_signs_the_persons(tc: TestCase) {
    let mut first = tc.draw(made());
    authors(&mut first);
    let later = tc.draw(
        gs::vecs(hegel::compose!(|tc| {
            Made {
                theirs: tc.draw(gs::booleans()),
                private: false,
            }
        }))
        .max_size(3),
    );
    let sign = tc.draw(gs::booleans());
    let origin = Origin::new(&format!("{PRIVATE}git.sign-on-push = {sign}\n"));
    let trunk = origin.project.trunk().unwrap();
    let chat = origin.project.add_workspace("chat", &trunk).unwrap();
    let dir = chat.root().to_owned();
    let ids = commit_all(&origin, &dir, "tau/chat", "c", &first);
    let onto = origin.project.upstream().unwrap().unwrap();
    let replayed = origin.project.replay(&ids, &onto);
    let branch = "refs/heads/tau/chat";
    if let Some(newest) = first.iter().rposition(|made| made.private) {
        let error = replayed.unwrap_err();
        assert!(matches!(error, VcsError::PrivateCommit { .. }), "{error}");
        assert!(
            error.to_string().contains(&subject(newest, &first[newest])),
            "{error}"
        );
        assert_eq!(origin.origin_ref(branch), None, "nothing sent");
        return;
    }
    let copies = replayed.unwrap();
    let head = origin
        .project
        .push_branch(origin.remote(), "tau/chat", None, copies.last().unwrap())
        .unwrap();
    assert_eq!(origin.origin_ref(branch), Some(head.clone()));
    if !sign {
        assert_eq!(&head, copies.last().unwrap(), "nothing rewritten");
    }
    let expected = |commits: &[Made]| -> Vec<(String, String, bool)> {
        commits
            .iter()
            .enumerate()
            .map(|(k, made)| {
                let email = if made.theirs { OTHER } else { PERSON };
                (subject(k, made), email.to_owned(), sign && !made.theirs)
            })
            .collect()
    };
    assert_eq!(origin.arrived(&format!("main..{branch}")), expected(&first));
    if later.is_empty() {
        return;
    }

    // Later commits, replayed onto what was pushed: only theirs change.
    let mut later = later;
    // The chat's `@` now was made by the person (see `commit_all`).
    authors(&mut later);
    let more = commit_all(&origin, &dir, "tau/chat", "d", &later);
    let copies = origin.project.replay(&more, &head).unwrap();
    let new_head = origin
        .project
        .push_branch(
            origin.remote(),
            "tau/chat",
            Some(&head),
            copies.last().unwrap(),
        )
        .unwrap();
    assert_eq!(origin.origin_ref(branch), Some(new_head.clone()));
    assert_eq!(origin.arrived(&format!("main..{head}")), expected(&first));
    assert_eq!(
        origin.arrived(&format!("{head}..{branch}")),
        expected(&later)
    );
}

/// `git.executable-path` is the `git` a push runs: a wrapper there
/// records each run, and a path to nothing fails the push, naming it.
#[test]
fn a_push_runs_the_configured_git() {
    let probe = tempfile::tempdir().unwrap();
    let log = probe.path().join("ran");
    let wrapper = probe.path().join("my-git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nfor arg; do echo \"$arg\"; done >> {}\nexec git \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(
            &wrapper,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let quoted = |path: &std::path::Path| {
        serde_json::to_string(path.to_str().unwrap()).unwrap()
    };
    let origin =
        Origin::new(&format!("git.executable-path = {}\n", quoted(&wrapper)));
    let main = origin.project.workspace_dir(DEFAULT_WORKSPACE);
    let trunk = origin.project.trunk_name().unwrap();
    commit_all(
        &origin,
        &main,
        &trunk,
        "m",
        &[Made {
            theirs: false,
            private: false,
        }],
    );
    let pushed = origin.project.push_trunk(origin.remote()).unwrap();
    assert_eq!(origin.origin_ref("main"), Some(pushed.to));
    let ran = std::fs::read_to_string(&log).unwrap();
    assert!(ran.lines().any(|line| line == "push"), "{ran}");

    let missing = probe.path().join("no-git");
    let origin =
        Origin::new(&format!("git.executable-path = {}\n", quoted(&missing)));
    let main = origin.project.workspace_dir(DEFAULT_WORKSPACE);
    let trunk = origin.project.trunk_name().unwrap();
    commit_all(
        &origin,
        &main,
        &trunk,
        "m",
        &[Made {
            theirs: false,
            private: false,
        }],
    );
    let error = origin.project.push_trunk(origin.remote()).unwrap_err();
    assert!(error.to_string().contains("no-git"), "{error}");
}

/// An operation that is never published, so loading the repository at
/// head never sees it: one commit, `aside`, on `parent`, in the
/// workspace at `main`. Returns the operation's id and the commit's, in
/// hex.
fn unpublished(main: &Path, parent: &str) -> (String, String) {
    use jj_lib::{
        backend::CommitId,
        config::{ConfigLayer, ConfigSource, StackedConfig},
        default_backend_factories::{
            default_backend_factories,
            default_working_copy_factories,
        },
        object_id::ObjectId as _,
        repo::Repo as _,
        settings::UserSettings,
        workspace::Workspace,
    };
    let mut config = StackedConfig::with_defaults();
    let mut user = ConfigLayer::empty(ConfigSource::User);
    user.set_value("user.name", "Other").unwrap();
    user.set_value("user.email", OTHER).unwrap();
    config.add_layer(user);
    let settings =
        UserSettings::from_config_and_home_dir(config, None).unwrap();
    let workspace = Workspace::load(
        &settings,
        main,
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    let repo =
        pollster::block_on(workspace.repo_loader().load_at_head()).unwrap();
    let parent = CommitId::try_from_hex(parent).unwrap();
    let tree = repo.store().get_commit(&parent).unwrap().tree();
    let mut tx = repo.start_transaction();
    let aside = pollster::block_on(
        tx.repo_mut()
            .new_commit(vec![parent], tree)
            .set_description("aside")
            .write(),
    )
    .unwrap();
    let operation = pollster::block_on(tx.write("aside")).unwrap();
    (operation.operation().id().hex(), aside.id().hex())
}

/// `git.private-commits` naming, through `at_operation()`, the
/// ancestors of a commit only an unpublished operation has: a trunk push
/// refuses exactly when that commit stands on one it would send, naming
/// it, as `jj git push` 0.46 does now that `at_operation()` loads the
/// operation's commits (jj 0.45 failed to evaluate it).
#[hegel::test(test_cases = 20)]
fn a_private_commits_operation_off_the_line_counts(tc: TestCase) {
    let count: usize = tc.draw(gs::integers().min_value(1).max_value(3));
    // On which of the commits the push would send the aside commit
    // stands, or on what the remote already has.
    let on: Option<usize> =
        tc.draw(gs::optional(gs::integers().max_value(count - 1)));
    let origin = Origin::new("");
    let main = origin.project.workspace_dir(DEFAULT_WORKSPACE);
    let trunk = origin.project.trunk_name().unwrap();
    let before = origin.origin_ref("main").unwrap();
    let commits = vec![
        Made {
            theirs: false,
            private: false,
        };
        count
    ];
    let ids = commit_all(&origin, &main, &trunk, "m", &commits);
    let parent = on.map_or(before.clone(), |k| ids[k].clone());
    let (operation, aside) = unpublished(&main, &parent);
    let config = origin.root.join("config").join("jj").join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str(&format!(
        "git.private-commits = 'at_operation({operation}, ::{aside})'\n"
    ));
    std::fs::write(&config, text).unwrap();
    // The config is read when a project opens.
    let project = ProjectRepo::open(
        origin.project.root(),
        Origin::identity_in(&origin.root, PERSON),
    )
    .unwrap();
    let result = project.push_trunk(origin.remote());
    match on {
        Some(k) => {
            let error = result.unwrap_err();
            assert!(matches!(error, VcsError::PrivateCommit { .. }), "{error}");
            let text = error.to_string();
            assert!(text.contains(&ids[k][..12]), "{text}");
            assert_eq!(origin.origin_ref("main"), Some(before), "nothing sent");
        }
        None => {
            let pushed = result.unwrap();
            assert_eq!(origin.origin_ref("main"), Some(pushed.to));
        }
    }
}
