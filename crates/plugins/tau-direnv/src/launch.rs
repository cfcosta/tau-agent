//! How a command starts in a workspace: through `direnv exec` once its
//! environment loaded and the person allowed it, else as it is
//! ([`launch_for`], pure); and the direnv tau runs, with the variables
//! every call of it gets ([`Direnv`]).

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

use sha2::{Digest as _, Sha256};
use tau_agent::launch::Launch;

/// direnv, and where tau keeps what it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Direnv {
    /// The `direnv` program.
    pub program: PathBuf,
    /// tau's configuration directory, `DIRENV_CONFIG` for every call
    /// (`crate::config`).
    pub config: PathBuf,
    /// The person's own configuration directory, which tau's mirrors;
    /// none when they have none.
    pub user_config: Option<PathBuf>,
    /// Where each workspace's `.direnv` goes (`direnv_layout_dir`),
    /// outside the workspace: tau commits everything a workspace holds.
    pub layouts: PathBuf,
    /// Variables every call gets besides those: for tests, direnv's own
    /// data directory.
    pub env: Vec<(OsString, OsString)>,
}

impl Direnv {
    /// direnv on `PATH`, keeping tau's files under `dir`; none when it is
    /// not installed.
    pub fn find(dir: &Path) -> Option<Self> {
        let path = std::env::var_os("PATH")?;
        let program = std::env::split_paths(&path)
            .map(|dir| dir.join("direnv"))
            .find(|candidate| is_executable(candidate))?;
        Some(Self::at(program, dir))
    }

    /// The direnv at `program`, keeping tau's files under `dir`, with the
    /// person's configuration where direnv finds it.
    pub fn at(program: PathBuf, dir: &Path) -> Self {
        Self {
            program,
            config: dir.join("config"),
            user_config: crate::config::user_dir().filter(|dir| dir.is_dir()),
            layouts: dir.join("layouts"),
            env: Vec::new(),
        }
    }

    /// The variables every call of direnv for `workspace` gets: tau's
    /// configuration, direnv's own messages silenced, and the
    /// workspace's layout directory.
    pub fn env(&self, workspace: &Path) -> Vec<(OsString, OsString)> {
        let mut env = vec![
            ("DIRENV_CONFIG".into(), self.config.clone().into_os_string()),
            ("DIRENV_LOG_FORMAT".into(), OsString::new()),
            (
                "direnv_layout_dir".into(),
                self.layout(workspace).into_os_string(),
            ),
        ];
        env.extend(self.env.iter().cloned());
        env
    }

    /// Where `workspace`'s `.direnv` goes: a directory of its own, named
    /// by its path.
    pub fn layout(&self, workspace: &Path) -> PathBuf {
        let hash = Sha256::digest(workspace.as_os_str().as_encoded_bytes());
        let hex: String =
            hash[..8].iter().map(|b| format!("{b:02x}")).collect();
        self.layouts.join(hex)
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Where a workspace's environment stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not looked at yet.
    Unknown,
    /// No direnv, no `.envrc`: nothing to load.
    Absent,
    /// The person has not said whether tau loads the repository's
    /// `.envrc`.
    Asking,
    Loading,
    Ready,
    Failed {
        status: String,
        output: String,
    },
    /// The person denied it with `direnv deny`.
    Denied,
    /// The person said commands run without it.
    Off,
}

impl Status {
    /// Whether a command waits on it: until the person answers, or it
    /// loads.
    pub fn waits(&self) -> bool {
        matches!(self, Self::Unknown | Self::Asking | Self::Loading)
    }
}

/// How a command in `workspace` starts: through `direnv exec` when there
/// is a direnv, the person allowed the repository's `.envrc` (`allowed`)
/// and the workspace's environment loaded; as it is otherwise.
pub fn launch_for(
    direnv: Option<&Direnv>,
    allowed: Option<bool>,
    status: &Status,
    workspace: &Path,
) -> Launch {
    match (direnv, allowed, status) {
        (Some(direnv), Some(true), Status::Ready) => Launch {
            prefix: vec![
                direnv.program.clone().into_os_string(),
                "exec".into(),
                workspace.as_os_str().to_owned(),
            ],
            env: direnv.env(workspace),
        },
        _ => Launch::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn direnv() -> Direnv {
        Direnv {
            program: "/bin/direnv".into(),
            config: "/tau/config".into(),
            user_config: None,
            layouts: "/tau/layouts".into(),
            env: Vec::new(),
        }
    }

    /// Every case of direnv × the person's answer × the workspace's
    /// status: only an installed direnv, an allowed repository and a
    /// loaded workspace start commands through `direnv exec`.
    #[test]
    fn commands_go_through_direnv_only_when_allowed_and_loaded() {
        let failed = Status::Failed {
            status: "direnv exited 1".into(),
            output: String::new(),
        };
        let statuses = [
            Status::Unknown,
            Status::Absent,
            Status::Asking,
            Status::Loading,
            Status::Ready,
            failed,
            Status::Denied,
            Status::Off,
        ];
        let workspace = Path::new("/tau/repo/runs/fix");
        let installed = direnv();
        let through = Launch {
            prefix: vec![
                "/bin/direnv".into(),
                "exec".into(),
                "/tau/repo/runs/fix".into(),
            ],
            env: vec![
                ("DIRENV_CONFIG".into(), "/tau/config".into()),
                ("DIRENV_LOG_FORMAT".into(), "".into()),
                (
                    "direnv_layout_dir".into(),
                    installed.layout(workspace).into_os_string(),
                ),
            ],
        };
        for direnv in [None, Some(&installed)] {
            for allowed in [None, Some(false), Some(true)] {
                for status in &statuses {
                    let expected = match (direnv.is_some(), allowed, status) {
                        (true, Some(true), Status::Ready) => through.clone(),
                        _ => Launch::default(),
                    };
                    assert_eq!(
                        launch_for(direnv, allowed, status, workspace),
                        expected,
                        "{:?} {allowed:?} {status:?}",
                        direnv.map(|d| &d.program)
                    );
                }
            }
        }
    }

    /// Each workspace has a layout directory of its own, under tau's.
    #[test]
    fn layouts_are_per_workspace() {
        let direnv = direnv();
        let a = direnv.layout(Path::new("/tau/repo/main"));
        let b = direnv.layout(Path::new("/tau/repo/runs/fix"));
        assert_ne!(a, b);
        assert!(a.starts_with("/tau/layouts") && b.starts_with("/tau/layouts"));
        assert_eq!(a, direnv.layout(Path::new("/tau/repo/main")));
    }
}
