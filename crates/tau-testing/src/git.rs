//! Git for test fixtures, kept apart from the user's own settings.

use std::{
    path::Path,
    process::{Command, Output},
};

/// `git` in `dir` with a fixed author and default branch, and none of
/// the user's or the system's settings (signing, hooks, `gc`). Local
/// clones of file URLs are allowed.
pub fn command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main", "-c", "gc.auto=0"])
        .args(["-c", "protocol.file.allow=always"])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// Runs `git args` in `dir`, whatever its exit status.
pub fn output(dir: &Path, args: &[&str]) -> Output {
    command(dir).args(args).output().expect("git runs")
}

/// Runs `git args` in `dir`, which must succeed, and returns what it
/// printed, trimmed.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = output(dir, args);
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout)
        .expect("git prints UTF-8")
        .trim()
        .to_owned()
}
