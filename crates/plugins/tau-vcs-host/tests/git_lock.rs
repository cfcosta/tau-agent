//! tau and the jj CLI never write a repository's Git store at once
//! (`docs/reference/vcs.md`, "Threading"): tau takes the lock jj-cli
//! 0.46.0 takes around its Git import and export,
//! `.jj/repo/git_import_export.lock`, after its own `tau.lock` and
//! before a working copy's lock, as the CLI takes it.
//!
//! Who waits on which lock is read from `/proc/locks`, which lists each
//! `flock` and each process blocked on one, so neither test depends on
//! timing: each waits until the kernel shows the other side blocked.
//! The second test needs the jj CLI of jj-lib's version, and says it did
//! not run without it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "common/jj_cli.rs"]
mod jj_cli;
#[path = "common/ops.rs"]
mod ops;

use std::{
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use jj_lib::lock::FileLock;
use serde_json::json;
use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
use tau_testing::block_on_io as block_on;
use tau_vcs_host::{DEFAULT_WORKSPACE, Identity, ProjectRepo, Vcs, VcsPlugin};

/// jj-cli's path, from its `lock_git_import_export`.
fn git_lock(workspace: &Path) -> PathBuf {
    workspace.join(".jj/repo/git_import_export.lock")
}

/// Waits until `/proc/locks` shows process `pid` blocked on the `flock`
/// of the file at `path`.
fn wait_blocked(pid: u32, path: &Path) {
    let inode = std::fs::metadata(path).unwrap().ino();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let locks = std::fs::read_to_string("/proc/locks").unwrap();
        let blocked = locks.lines().any(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            // `1: -> FLOCK  ADVISORY  WRITE <pid> <maj>:<min>:<inode> ...`
            fields.get(1) == Some(&"->")
                && fields.get(2) == Some(&"FLOCK")
                && fields.get(5) == Some(&pid.to_string().as_str())
                && fields.get(6).and_then(|dev| dev.rsplit(':').next())
                    == Some(inode.to_string().as_str())
        });
        if blocked {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "process {pid} never blocked on {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A project, made by tau, whose main workspace has a Git store beside
/// it, as jj-cli looks for one before it takes the lock.
fn project(home: &Path) -> ProjectRepo {
    let project = ProjectRepo::init(
        home.join("p"),
        Identity::default(),
        &[("a.txt".into(), "one\n".into())],
    )
    .unwrap();
    assert!(
        project
            .workspace_dir(DEFAULT_WORKSPACE)
            .join(".git")
            .is_dir()
    );
    project
}

/// While something holds jj-cli's lock, as a `jj` command does, tau's
/// writes wait for it, and write once it is let go.
#[test]
fn tau_waits_while_the_jj_cli_holds_its_git_lock() {
    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    let main = project.workspace_dir(DEFAULT_WORKSPACE);
    let mut log = ops::OpLog::open(&main.join(".jj/repo"));
    log.new_ops();
    let before = log.head();

    let held = FileLock::lock(git_lock(&main)).unwrap();
    let trunk = project.trunk().unwrap();
    let adding = {
        let project = project.clone();
        std::thread::spawn(move || project.add_workspace("r", &trunk).map(drop))
    };
    wait_blocked(std::process::id(), &git_lock(&main));
    assert_eq!(log.head().id(), before.id(), "tau wrote under jj's lock");
    assert!(!adding.is_finished());

    drop(held);
    adding.join().unwrap().unwrap();
    let new = log.new_ops();
    assert_eq!(new.len(), 1, "one operation adds the workspace");
    ops::assert_taus(&new[0], "add_workspace");
}

/// While tau holds the lock, between its snapshot's start and the
/// operation it writes, a `jj` command in the colocated workspace waits
/// for it. tau holds it there for as long as the working copy it
/// snapshots stays locked, here by the test.
#[test]
fn the_jj_cli_waits_while_tau_holds_its_git_lock() {
    let Some(jj) = jj_cli::matching_jj() else {
        eprintln!("no jj CLI of jj-lib's version: did not compare");
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let home = std::fs::canonicalize(home.path()).unwrap();
    let project = project(&home);
    let main = project.workspace_dir(DEFAULT_WORKSPACE);
    let trunk = project.trunk().unwrap();
    let run = project.workspace_dir("r");
    project.add_workspace("r", &trunk).unwrap();
    std::fs::write(run.join("b.txt"), "two\n").unwrap();

    // The run's working copy, locked as jj-lib locks it: tau's snapshot
    // takes tau.lock and the Git lock, then waits here.
    let working_copy = run.join(".jj/working_copy/working_copy.lock");
    let held = FileLock::lock(working_copy.clone()).unwrap();
    let status = std::thread::spawn(move || {
        let vcs = block_on(Vcs::open(&run, Identity::default())).unwrap();
        let tools = VcsPlugin::new(vcs).tools();
        let tool = tools.iter().find(|t| t.name() == "vcs_status").unwrap();
        block_on(tool.call(json!({}), ToolCtx::detached())).map(drop)
    });
    wait_blocked(std::process::id(), &working_copy);
    assert!(
        FileLock::try_lock(git_lock(&main)).unwrap().is_none(),
        "tau holds the Git lock while it waits for the working copy"
    );

    let mut cli = Command::new(&jj)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("JJ_USER", "Person")
        .env("JJ_EMAIL", "person@localhost")
        .current_dir(&main)
        .args(["new"])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    wait_blocked(cli.id(), &git_lock(&main));
    assert!(cli.try_wait().unwrap().is_none(), "jj ran under tau's lock");

    drop(held);
    status.join().unwrap().unwrap();
    let out = cli.wait_with_output().unwrap();
    assert!(out.status.success(), "jj new: {out:?}");
}
