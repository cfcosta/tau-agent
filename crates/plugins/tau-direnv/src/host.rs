//! The host half: what the person allowed, each workspace's environment
//! as it loads, and the launcher commands start through.
//!
//! One [`Host`] serves every repository. Each workspace a run works in
//! is looked at once: without an `.envrc` (or without direnv) nothing
//! happens; with one, the person is asked, once per repository, then
//! `direnv export json` loads it in the background. A command waits in
//! [`Launcher::launch`] until the workspace settles, then starts through
//! `direnv exec` when it loaded ([`crate::launch::launch_for`]).
//!
//! Each change of a workspace reaches the runs working in it as a
//! [`Record`], stored with the run and pushed to the interface at once
//! (`HostCx::publish`): no run event flows while a command waits.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use serde_json::Value;
use tau_agent::{
    event::RunEvent,
    launch::{Launch, Launcher},
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::RunId,
};
use tau_ui_plugin::{HostCx, PluginHost, RepoCtx};
use tokio::sync::watch;

use crate::{
    NAME,
    Record,
    RepoData,
    Settings,
    launch::{Direnv, Status, launch_for},
};

/// The file direnv loads.
pub const ENVRC: &str = ".envrc";

/// How much of an `.envrc` the question shows.
const ENVRC_SHOWN: usize = 4096;

/// How many of direnv's last lines a failure shows.
const OUTPUT_LINES: usize = 12;

/// tau-direnv on the host: shared by every repository and run.
#[derive(Clone)]
pub struct Host(Arc<Inner>);

struct Inner {
    /// None when direnv is not installed: the plugin is off.
    direnv: Option<Direnv>,
    cx: HostCx,
    /// What the person decided, by repository.
    allowed: Mutex<BTreeMap<String, bool>>,
    /// Each repository's directory, which holds its workspaces: what
    /// tau's whitelist allows.
    roots: Mutex<BTreeMap<String, PathBuf>>,
    workspaces: Mutex<HashMap<PathBuf, Workspace>>,
    /// Records to store with runs, in order, on a thread of their own.
    records: std::sync::mpsc::Sender<(RunId, Record)>,
}

/// A workspace tau looked at.
struct Workspace {
    repo: String,
    status: watch::Sender<Status>,
    /// The runs working in it, which hear of its changes.
    runs: Vec<RunId>,
}

impl PluginHost for Host {
    fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let direnv = Direnv::find(&cx.plugin_dir(NAME));
        Ok(Self::with_direnv(cx, direnv))
    }
}

impl Host {
    /// A host running `direnv` (none: the plugin is off), for tests and
    /// for [`PluginHost::new`].
    pub fn with_direnv(cx: &HostCx, direnv: Option<Direnv>) -> Self {
        let settings: Settings = cx.settings(NAME);
        let (records, receive) = std::sync::mpsc::channel::<(RunId, Record)>();
        let publisher = cx.clone();
        std::thread::Builder::new()
            .name("tau-direnv".into())
            .spawn(move || {
                for (run, record) in receive {
                    let body = serde_json::to_value(&record)
                        .expect("a record serializes");
                    if let Err(error) = publisher.publish(&run, NAME, &body) {
                        eprintln!("{NAME}: cannot store a record: {error:#}");
                    }
                }
            })
            .expect("a thread starts");
        let inner = Inner {
            direnv,
            cx: cx.clone(),
            allowed: Mutex::new(settings.repos),
            roots: Mutex::new(
                cx.repos
                    .iter()
                    .map(|repo| (repo.name.clone(), repo.workspaces.clone()))
                    .collect(),
            ),
            workspaces: Mutex::default(),
            records,
        };
        inner.write_config();
        Self(Arc::new(inner))
    }

    /// Whether direnv is installed.
    pub fn installed(&self) -> bool {
        self.0.direnv.is_some()
    }

    /// What commands in `repo` start through; none without direnv.
    pub fn launcher(&self, repo: &RepoCtx) -> Option<Arc<dyn Launcher>> {
        self.0.direnv.as_ref()?;
        self.0.know(repo);
        Some(Arc::new(RepoEnvironment {
            host: self.clone(),
            repo: repo.name.clone(),
        }))
    }

    /// The agent plugin for a run of `repo` in `dir`.
    pub fn plugin(&self, repo: &RepoCtx, dir: PathBuf) -> DirenvPlugin {
        DirenvPlugin {
            host: self.clone(),
            repo: repo.name.clone(),
            dir,
        }
    }

    /// What the repository's menu shows.
    pub fn repo_data(&self, repo: &RepoCtx) -> RepoData {
        RepoData {
            envrc: repo.checkout.join(ENVRC).is_file(),
            direnv: self.installed(),
        }
    }

    /// Where `dir`'s environment stands.
    pub fn status(&self, dir: &Path) -> Status {
        self.0
            .workspaces
            .lock()
            .expect("not poisoned")
            .get(dir)
            .map_or(Status::Unknown, |workspace| {
                workspace.status.borrow().clone()
            })
    }

    /// The person decided whether `repo`'s `.envrc` loads: kept, tau's
    /// configuration written again, and its workspaces load or stop.
    pub fn decide(&self, repo: &str, load: bool) -> anyhow::Result<()> {
        let repos = {
            let mut allowed = self.0.allowed.lock().expect("not poisoned");
            allowed.insert(repo.to_owned(), load);
            allowed.clone()
        };
        self.0.cx.save_settings(NAME, &Settings { repos })?;
        self.0.write_config();
        let dirs: Vec<PathBuf> = self
            .0
            .workspaces
            .lock()
            .expect("not poisoned")
            .iter()
            .filter(|(_, workspace)| workspace.repo == repo)
            .filter(|(_, workspace)| {
                *workspace.status.borrow() != Status::Absent
            })
            .map(|(dir, _)| dir.clone())
            .collect();
        for dir in dirs {
            if load {
                self.0.load(&self.0, &dir);
            } else {
                self.0.set(&dir, Status::Off);
            }
        }
        Ok(())
    }

    /// Loads the environment of the workspace `run` works in again.
    pub fn reload(&self, run: &str) -> anyhow::Result<()> {
        let dir = self
            .0
            .workspaces
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|(_, workspace)| {
                workspace.runs.iter().any(|id| &*id.0 == run)
            })
            .map(|(dir, _)| dir.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("tau has not loaded this run's environment")
            })?;
        self.0.load(&self.0, &dir);
        Ok(())
    }

    /// `run` works in `dir` of `repo`: it hears of the workspace's
    /// changes from now on, and of where it stands now, unless it
    /// loaded already or has nothing to load.
    fn attend(&self, run: RunId, repo: &str, dir: &Path) {
        let known = {
            let mut workspaces =
                self.0.workspaces.lock().expect("not poisoned");
            let workspace = self.0.entry(&mut workspaces, repo, dir);
            if !workspace.runs.contains(&run) {
                workspace.runs.push(run.clone());
            }
            *workspace.status.borrow() != Status::Unknown
        };
        // Looked at the first time, the run heard as it changed.
        let status = self.0.look(&self.0, repo, dir);
        if known
            && !matches!(
                status,
                Status::Ready | Status::Absent | Status::Unknown
            )
            && let Some(record) = record_of(&status, repo, dir)
        {
            let _ = self.0.records.send((run, record));
        }
    }
}

impl Inner {
    /// Knows `repo`'s directory, writing tau's configuration again when
    /// it is new and allowed.
    fn know(&self, repo: &RepoCtx) {
        let changed = self
            .roots
            .lock()
            .expect("not poisoned")
            .insert(repo.name.clone(), repo.workspaces.clone())
            .is_none_or(|before| before != repo.workspaces);
        if changed {
            self.write_config();
        }
    }

    /// Writes tau's direnv configuration, allowing the repositories the
    /// person allowed.
    fn write_config(&self) {
        let Some(direnv) = &self.direnv else {
            return;
        };
        let allowed = self.allowed.lock().expect("not poisoned").clone();
        let roots: Vec<PathBuf> = self
            .roots
            .lock()
            .expect("not poisoned")
            .iter()
            .filter(|(name, _)| allowed.get(*name) == Some(&true))
            .map(|(_, root)| root.clone())
            .collect();
        if let Err(error) = crate::config::write(
            &direnv.config,
            direnv.user_config.as_deref(),
            &roots,
        ) {
            eprintln!(
                "{NAME}: cannot write tau's direnv configuration: {error}"
            );
        }
    }

    fn entry<'a>(
        &self,
        workspaces: &'a mut HashMap<PathBuf, Workspace>,
        repo: &str,
        dir: &Path,
    ) -> &'a mut Workspace {
        workspaces
            .entry(dir.to_owned())
            .or_insert_with(|| Workspace {
                repo: repo.to_owned(),
                status: watch::Sender::new(Status::Unknown),
                runs: Vec::new(),
            })
    }

    /// Where `dir` stands, looking at it the first time: nothing to
    /// load, the person to ask, or its load started.
    fn look(&self, me: &Arc<Self>, repo: &str, dir: &Path) -> Status {
        let first = {
            let mut workspaces = self.workspaces.lock().expect("not poisoned");
            let workspace = self.entry(&mut workspaces, repo, dir);
            *workspace.status.borrow() == Status::Unknown
        };
        if first {
            if self.direnv.is_none() || !dir.join(ENVRC).is_file() {
                self.set(dir, Status::Absent);
            } else {
                match self.allowed.lock().expect("not poisoned").get(repo) {
                    None => self.set(dir, Status::Asking),
                    Some(false) => self.set(dir, Status::Off),
                    Some(true) => self.load(me, dir),
                }
            }
        }
        self.watch(dir).borrow().clone()
    }

    fn watch(&self, dir: &Path) -> watch::Receiver<Status> {
        self.workspaces.lock().expect("not poisoned")[dir]
            .status
            .subscribe()
    }

    /// Changes where `dir` stands, telling the runs in it.
    fn set(&self, dir: &Path, status: Status) {
        let (repo, runs) = {
            let workspaces = self.workspaces.lock().expect("not poisoned");
            let Some(workspace) = workspaces.get(dir) else {
                return;
            };
            workspace.status.send_replace(status.clone());
            (workspace.repo.clone(), workspace.runs.clone())
        };
        if let Some(record) = record_of(&status, &repo, dir) {
            for run in runs {
                let _ = self.records.send((run, record.clone()));
            }
        }
    }

    /// Loads `dir`'s environment in the background, unless it is
    /// loading already.
    fn load(&self, me: &Arc<Self>, dir: &Path) {
        let Some(direnv) = self.direnv.clone() else {
            return;
        };
        {
            let workspaces = self.workspaces.lock().expect("not poisoned");
            let Some(workspace) = workspaces.get(dir) else {
                return;
            };
            if *workspace.status.borrow() == Status::Loading {
                return;
            }
        }
        self.set(dir, Status::Loading);
        let (me, dir) = (me.clone(), dir.to_owned());
        self.cx.runtime.spawn(async move {
            let status = load(&direnv, &dir).await;
            // The person may have said no meanwhile.
            if *me.watch(&dir).borrow() == Status::Loading {
                me.set(&dir, status);
            }
        });
    }
}

/// What the runs in `dir` hear of `status`; nothing for the states no
/// one sees.
fn record_of(status: &Status, repo: &str, dir: &Path) -> Option<Record> {
    Some(match status {
        Status::Unknown | Status::Absent => return None,
        Status::Asking => Record::Asked {
            repo: repo.to_owned(),
            envrc: envrc_text(dir),
        },
        Status::Loading => Record::Loading { since: now_ms() },
        Status::Ready => Record::Loaded,
        Status::Failed { status, output } => Record::Failed {
            status: status.clone(),
            output: output.clone(),
        },
        Status::Denied => Record::Denied,
        Status::Off => Record::Off,
    })
}

/// The `.envrc` in `dir`, as much as the question shows.
fn envrc_text(dir: &Path) -> String {
    let text = std::fs::read_to_string(dir.join(ENVRC)).unwrap_or_default();
    if text.len() <= ENVRC_SHOWN {
        return text;
    }
    let mut end = ENVRC_SHOWN;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n…", &text[..end])
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// Loads `dir`'s environment with `direnv export json`: ready, failed
/// with what direnv said, or denied when the person ran `direnv deny`.
pub async fn load(direnv: &Direnv, dir: &Path) -> Status {
    let command = |args: &[&str]| {
        let mut command = tokio::process::Command::new(&direnv.program);
        command
            .args(args)
            .current_dir(dir)
            .envs(direnv.env(dir))
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    };
    if let Ok(out) = command(&["status", "--json"]).output().await
        && out.status.success()
        && let Ok(status) = serde_json::from_slice::<Value>(&out.stdout)
    {
        // direnv's own answer: 0 allowed, 1 not allowed, 2 denied.
        match status["state"]["foundRC"]["allowed"].as_u64() {
            Some(2) => return Status::Denied,
            Some(1) => {
                return Status::Failed {
                    status: "direnv did not allow it".into(),
                    output: format!(
                        "{} is not in tau's whitelist ({})",
                        dir.join(ENVRC).display(),
                        direnv.config.join("direnv.toml").display()
                    ),
                };
            }
            _ => {}
        }
    }
    match command(&["export", "json"])
        .stdout(Stdio::null())
        .output()
        .await
    {
        Ok(out) if out.status.success() => Status::Ready,
        Ok(out) => Status::Failed {
            status: match out.status.code() {
                Some(code) => format!("direnv exited {code}"),
                None => "direnv was killed".into(),
            },
            output: tail(&plain(&String::from_utf8_lossy(&out.stderr))),
        },
        Err(error) => Status::Failed {
            status: "direnv did not start".into(),
            output: error.to_string(),
        },
    }
}

/// `text` without its terminal escape sequences (direnv colors its
/// errors).
pub fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// The last lines of `text`, as a card shows them.
fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.trim_end().lines().collect();
    lines[lines.len().saturating_sub(OUTPUT_LINES)..].join("\n")
}

/// A repository's launcher: commands in its workspaces start through
/// `direnv exec` once allowed and loaded.
struct RepoEnvironment {
    host: Host,
    repo: String,
}

#[async_trait]
impl Launcher for RepoEnvironment {
    async fn launch(&self, dir: &Path) -> Launch {
        let inner = &self.host.0;
        if inner.direnv.is_none() || !dir.join(ENVRC).is_file() {
            return Launch::default();
        }
        inner.look(inner, &self.repo, dir);
        let mut watch = inner.watch(dir);
        let status = match watch.wait_for(|status| !status.waits()).await {
            Ok(status) => status.clone(),
            Err(_) => return Launch::default(),
        };
        let allowed = inner
            .allowed
            .lock()
            .expect("not poisoned")
            .get(&self.repo)
            .copied();
        launch_for(inner.direnv.as_ref(), allowed, &status, dir)
    }
}

/// tau-direnv's agent plugin: the run hears of its workspace's
/// environment from its first turn.
pub struct DirenvPlugin {
    host: Host,
    repo: String,
    dir: PathBuf,
}

#[async_trait]
impl Plugin for DirenvPlugin {
    fn name(&self) -> &str {
        NAME
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(Attend {
            host: self.host.clone(),
            repo: self.repo.clone(),
            dir: self.dir.clone(),
            done: false,
        }))
    }
}

struct Attend {
    host: Host,
    repo: String,
    dir: PathBuf,
    done: bool,
}

#[async_trait]
impl PluginRun for Attend {
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        // At its first turn: the run is stored by then, so its records
        // can be.
        let RunEvent::TurnStart { run, .. } = event else {
            return;
        };
        if self.done || run != &ctx.run {
            return;
        }
        self.done = true;
        self.host.attend(run.clone(), &self.repo, &self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_are_dropped_and_the_tail_kept() {
        assert_eq!(
            plain("\u{1b}[31mdirenv: error exit status 1\u{1b}[0m"),
            "direnv: error exit status 1"
        );
        let long: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let kept = tail(&long);
        assert_eq!(kept.lines().count(), OUTPUT_LINES);
        assert!(kept.ends_with("line 20"));
    }
}
