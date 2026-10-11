//! Where an agent's commands run: inside `bwrap` (bubblewrap), which
//! shows them their workspace, the system's programs and the network,
//! and nothing of the person's (ADR 0037, "`bash` runs in a sandbox").
//!
//! Each command gets a sandbox of its own:
//!
//! - its workspace, read-write, at its own path, where it starts;
//! - `/nix`, `/usr`, `/bin`, `/sbin`, `/lib`, `/lib64`, `/etc`,
//!   `/run/current-system` and `/run/systemd/resolve`, read-only, where
//!   they exist: the shell, the system's programs, their libraries and
//!   the host's settings (DNS, certificates, users). They hold nothing
//!   the person's own processes cannot read;
//! - a private `/tmp`, `/dev` and `/proc`;
//! - a home of the agent's own, kept under tau's data across commands,
//!   at [`HOME`];
//! - the network, and no other namespace of the host's.
//!
//! The person's home, their other repositories, tau's own data (its
//! settings, secrets and runs) and `/run/user` are not there. Variables
//! of tau's own environment are dropped but for [`KEPT`]: what a command
//! sets for itself (its terminal's) passes. A command dies with tau.

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
};

use async_trait::async_trait;
use tau_agent::launch::{Launch, Launcher};

/// Where an agent's home is, inside its sandbox.
pub const HOME: &str = "/home/agent";

/// The host's paths a sandbox sees, read-only, where they exist.
pub const SYSTEM: [&str; 9] = [
    "/usr",
    "/bin",
    "/sbin",
    "/lib",
    "/lib64",
    "/etc",
    "/run/current-system",
    "/run/systemd/resolve",
    "/nix",
];

/// The variables of tau's own environment a command keeps: how it
/// finds programs, its locale and terminal, and the certificates it
/// trusts. `LC_*` are kept too.
pub const KEPT: [&str; 17] = [
    "PATH",
    "LANG",
    "LANGUAGE",
    "TZ",
    "TZDIR",
    "TERM",
    "COLORTERM",
    "PAGER",
    "GIT_PAGER",
    "MANPAGER",
    "LESS",
    "SHELL",
    "USER",
    "LOGNAME",
    "LOCALE_ARCHIVE",
    "SSL_CERT_FILE",
    "NIX_SSL_CERT_FILE",
];

/// Whether a command keeps `name` of tau's environment.
pub fn kept(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| KEPT.contains(&name) || name.starts_with("LC_"))
}

/// `bwrap`'s words for a command working in `workspace`, with the
/// agent's home at `home` on the host, dropping every variable of
/// `env` (tau's environment) that is not [`kept`]. The program and its
/// arguments come after them.
pub fn words<'a>(
    bwrap: &Path,
    home: &Path,
    workspace: &Path,
    env: impl IntoIterator<Item = &'a OsStr>,
) -> Vec<OsString> {
    let mut words: Vec<OsString> = vec![
        bwrap.into(),
        "--unshare-all".into(),
        "--share-net".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
    ];
    for path in SYSTEM {
        words.extend(["--ro-bind-try".into(), path.into(), path.into()]);
    }
    words.extend([
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        // Before the binds: a workspace under /tmp stays seen.
        "--tmpfs".into(),
        "/tmp".into(),
        "--bind".into(),
        home.into(),
        HOME.into(),
        "--bind".into(),
        workspace.into(),
        workspace.into(),
        "--chdir".into(),
        workspace.into(),
        "--setenv".into(),
        "HOME".into(),
        HOME.into(),
        "--setenv".into(),
        "TMPDIR".into(),
        "/tmp".into(),
    ]);
    for name in env {
        if !kept(name) && name != "HOME" && name != "TMPDIR" {
            words.extend(["--unsetenv".into(), name.to_owned()]);
        }
    }
    words.push("--".into());
    words
}

/// The first `bwrap` on `PATH`, if there is one.
pub fn find_bwrap() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join("bwrap"))
        .find(|candidate| candidate.is_file())
}

/// What an agent's commands start through: `bwrap`, with the agent's
/// home at `home`.
#[derive(Debug, Clone)]
pub struct Sandbox {
    pub bwrap: PathBuf,
    pub home: PathBuf,
}

#[async_trait]
impl Launcher for Sandbox {
    async fn launch(&self, dir: &Path) -> Launch {
        // bwrap binds only what exists: the home is made the first
        // time. Failing that, bwrap fails the command and says why.
        if let Err(error) = tokio::fs::create_dir_all(&self.home).await {
            eprintln!(
                "tau-agents: cannot make {}: {error}",
                self.home.display()
            );
        }
        let env: Vec<OsString> =
            std::env::vars_os().map(|(name, _)| name).collect();
        Launch {
            prefix: words(
                &self.bwrap,
                &self.home,
                dir,
                env.iter().map(OsString::as_os_str),
            ),
            env: Vec::new(),
        }
    }
}
