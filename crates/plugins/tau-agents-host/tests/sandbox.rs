//! Where an agent's commands run (ADR 0037): `bwrap`'s words for a
//! command, and, when `bwrap` is on `PATH` (skipped otherwise), a real
//! command in a sandbox: it writes its workspace, reaches neither the
//! person's files nor tau's environment, and resolves names.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    ffi::{OsStr, OsString},
    path::Path,
};

use tau_agent::launch::Launcher;
use tau_agents_host::sandbox::{HOME, SYSTEM, Sandbox, find_bwrap, words};
use tau_testing::block_on_io;

fn strings(words: &[OsString]) -> Vec<String> {
    words
        .iter()
        .map(|word| word.to_string_lossy().into_owned())
        .collect()
}

/// Whether `words` hold `run` as consecutive words.
fn has(words: &[String], run: &[&str]) -> bool {
    words
        .windows(run.len())
        .any(|window| window.iter().zip(run).all(|(a, b)| a == b))
}

fn at(words: &[String], run: &[&str]) -> usize {
    words
        .windows(run.len())
        .position(|window| window.iter().zip(run).all(|(a, b)| a == b))
        .unwrap_or_else(|| panic!("{run:?} in {words:?}"))
}

/// The sandbox shares only the network, sees the system read-only and
/// its workspace read-write, starts there, with the agent's home, and
/// ends its words with `--`.
#[test]
fn a_command_sees_the_system_its_workspace_and_its_home() {
    let words = strings(&words(
        Path::new("/bin/bwrap"),
        Path::new("/tau/plugins/tau-agents/homes/greet"),
        Path::new("/tmp/tau/agents/greet/workspaces/main"),
        std::iter::empty(),
    ));
    assert_eq!(words[0], "/bin/bwrap");
    assert!(has(&words, &["--unshare-all", "--share-net"]));
    assert!(has(&words, &["--die-with-parent"]));
    assert!(has(&words, &["--new-session"]));
    for path in SYSTEM {
        assert!(has(&words, &["--ro-bind-try", path, path]), "{path}");
    }
    let workspace = "/tmp/tau/agents/greet/workspaces/main";
    assert!(has(&words, &["--bind", workspace, workspace]));
    assert!(has(&words, &["--chdir", workspace]));
    assert!(has(
        &words,
        &["--bind", "/tau/plugins/tau-agents/homes/greet", HOME]
    ));
    assert!(has(&words, &["--setenv", "HOME", HOME]));
    // A workspace under /tmp is bound over the private /tmp, not under.
    assert!(
        at(&words, &["--tmpfs", "/tmp"])
            < at(&words, &["--bind", workspace, workspace])
    );
    assert_eq!(words.last().map(String::as_str), Some("--"));
    assert!(!words.iter().any(|word| word.contains("/home/person")));
}

/// tau's own variables are dropped but for what a command needs to
/// find programs, speak its locale and trust certificates.
#[test]
fn taus_variables_are_dropped_but_for_the_kept_ones() {
    let env = [
        "PATH",
        "LANG",
        "LC_ALL",
        "TERM",
        "GITHUB_TOKEN",
        "AWS_SECRET_ACCESS_KEY",
        "XDG_RUNTIME_DIR",
        "HOME",
        "SSL_CERT_FILE",
    ];
    let words = strings(&words(
        Path::new("bwrap"),
        Path::new("/home-of-agent"),
        Path::new("/workspace"),
        env.iter().map(OsStr::new),
    ));
    for dropped in ["GITHUB_TOKEN", "AWS_SECRET_ACCESS_KEY", "XDG_RUNTIME_DIR"]
    {
        assert!(has(&words, &["--unsetenv", dropped]), "{dropped}");
    }
    for kept in ["PATH", "LANG", "LC_ALL", "TERM", "SSL_CERT_FILE", "HOME"] {
        assert!(!has(&words, &["--unsetenv", kept]), "{kept}");
    }
}

/// A real command in a real sandbox: it writes its workspace and its
/// home, cannot see a file next to its workspace (a stand-in for the
/// person's), has none of tau's own variables in its environment (here
/// `CARGO_MANIFEST_DIR`, which the test runner sets for every test),
/// and reads the host's DNS settings. `bwrap` is in the dev shell
/// (`flake.nix`), so its absence fails the test rather than skipping it.
#[test]
fn a_real_command_stays_in_its_sandbox() {
    let bwrap = find_bwrap().expect("bwrap is on PATH in the dev shell");
    assert!(
        std::env::var_os("CARGO_MANIFEST_DIR").is_some(),
        "the test runner sets CARGO_MANIFEST_DIR"
    );
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("agents/greet/workspaces/main");
    std::fs::create_dir_all(&workspace).unwrap();
    let person = dir.path().join("person");
    std::fs::create_dir_all(&person).unwrap();
    std::fs::write(person.join("secret"), "the person's").unwrap();
    let sandbox = Sandbox {
        bwrap,
        home: dir.path().join("homes/greet"),
    };
    let launch = block_on_io(sandbox.launch(&workspace));
    let script = format!(
        "echo made > made && echo kept > \"$HOME/kept\" && \
         if cat {secret} 2>/dev/null; then echo SEEN; fi; \
         echo home=$HOME; \
         echo manifest=${{CARGO_MANIFEST_DIR-dropped}}; \
         test -r /etc/resolv.conf && echo dns",
        secret = person.join("secret").display(),
    );
    let (program, args) = launch.argv("sh".as_ref(), ["-c", &script]);
    let out = std::process::Command::new(program)
        .args(args)
        .current_dir(&workspace)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("made")).unwrap(),
        "made\n"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("homes/greet/kept")).unwrap(),
        "kept\n"
    );
    assert!(!stdout.contains("SEEN"), "{stdout}");
    assert!(stdout.contains(&format!("home={HOME}")), "{stdout}");
    assert!(stdout.contains("manifest=dropped\n"), "{stdout}");
    if Path::new("/etc/resolv.conf").exists() {
        assert!(stdout.contains("dns"), "{stdout}");
    }
}
