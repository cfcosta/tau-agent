//! Each agent's `agent.luau`, read from its repository's trunk (ADR
//! 0037).
//!
//! The registry watches every agent's trunk. When it moves, the agent's
//! folder at the new commit is loaded and tested: a version whose tests
//! pass is live at once, and its triggers are what wake the agent. One
//! that does not load or whose tests fail leaves the version before
//! live, and the agent hears why, once, as an event, so it can fix it.
//! Nothing waits for the person: an agent changes itself.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tau_agents::{
    TriggerKind,
    Version,
    event::{Event, Kind},
};
use tau_luau_plugins_host::runtime::{Files, Modules, evaluate, run_tests};
use tau_vcs_host::{Identity, Project};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

use crate::{AGENT, clock::Clock, schedule::Schedule};

/// How often each agent's trunk is read for a change.
const POLL: Duration = Duration::from_secs(2);

/// How many versions of an agent its page lists.
const VERSIONS: usize = 5;

/// How long reading what an agent declares may take.
const DESCRIBE_LIMIT: Duration = Duration::from_secs(2);

/// The trigger name a version's events carry: the file that failed.
pub const VERSION_TRIGGER: &str = tau_agents::ENTRY;

/// What an agent's `agent.luau` declares, as `tau._describe_agent`
/// reads it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Declared {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<Declaration>,
}

/// One trigger as an agent declares it: its name and what it is.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Declaration {
    pub name: String,
    pub kind: TriggerKind,
}

/// The version of an agent whose triggers fire.
#[derive(Clone)]
pub struct Live {
    pub commit: String,
    pub declared: Declared,
    pub files: Files,
    pub modules: Arc<Modules>,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("commit", &self.commit)
            .field("declared", &self.declared)
            .finish_non_exhaustive()
    }
}

/// An agent as the registry last read it.
#[derive(Debug, Clone, Default)]
pub struct Read {
    /// Trunk's commit, once read.
    pub trunk: Option<String>,
    /// Its latest versions, newest first.
    pub versions: Vec<Version>,
    pub live: Option<Live>,
    /// Why no version is live: trunk's does not load, or its tests fail.
    /// `None` while it has no `agent.luau` yet.
    pub error: Option<String>,
    /// What it is for: what `agent.luau` says, else the first lines of
    /// its `AGENTS.md`.
    pub purpose: String,
}

/// What the registry keeps of each agent across restarts: its live
/// version, and the last failing version it told it about.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Kept {
    live: Option<String>,
    told: Option<String>,
}

/// The agents' versions. Clones share them.
#[derive(Clone)]
pub struct Registry(Arc<Shared>);

struct Shared {
    /// Where the agents' repositories are: one directory each.
    root: PathBuf,
    /// Where [`Kept`] is kept.
    kept_path: PathBuf,
    clock: Arc<dyn Clock>,
    agents: RwLock<BTreeMap<String, Read>>,
    projects: Mutex<BTreeMap<String, Project>>,
    /// Asks the interface to draw the catalog again.
    refresh: Box<dyn Fn() + Send + Sync>,
    /// Tells an agent a version of it failed: an event for its queue.
    failed: Tell,
}

/// What hands an agent, by name, an event to hear.
type Tell = Box<dyn Fn(&str, Event) + Send + Sync>;

impl Registry {
    /// A registry of the agents under `root`, keeping what it must
    /// across restarts at `kept`. `refresh` redraws the catalog; `failed`
    /// takes the event an agent hears when a version of it fails. Empty
    /// until it reads the agents' trunks.
    pub fn at(
        root: PathBuf,
        kept: PathBuf,
        clock: Arc<dyn Clock>,
        refresh: impl Fn() + Send + Sync + 'static,
        failed: impl Fn(&str, Event) + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Shared {
            root,
            kept_path: kept,
            clock,
            agents: RwLock::default(),
            projects: Mutex::default(),
            refresh: Box::new(refresh),
            failed: Box::new(failed),
        }))
    }

    /// Every agent as last read, by name.
    pub async fn agents(&self) -> BTreeMap<String, Read> {
        self.0.agents.read().await.clone()
    }

    /// `name`'s live version, if one is.
    pub async fn live(&self, name: &str) -> Option<Live> {
        self.0.agents.read().await.get(name)?.live.clone()
    }

    /// Reads every agent's trunk every [`POLL`], and reloads those whose
    /// trunk moved. Runs for the host's life.
    pub async fn watch(&self) {
        loop {
            self.scan().await;
            tokio::time::sleep(POLL).await;
        }
    }

    /// Reads the agents' directory and each agent's trunk once,
    /// reloading those whose trunk moved; an agent whose directory is
    /// gone is forgotten.
    pub async fn scan(&self) {
        let root = self.0.root.clone();
        let names = tokio::task::spawn_blocking(move || agent_dirs(&root))
            .await
            .unwrap_or_default();
        let gone: Vec<String> = {
            let mut agents = self.0.agents.write().await;
            let gone: Vec<String> = agents
                .keys()
                .filter(|name| !names.contains(name))
                .cloned()
                .collect();
            agents.retain(|name, _| names.contains(name));
            gone
        };
        self.0
            .projects
            .lock()
            .await
            .retain(|name, _| names.contains(name));
        let mut changed = !gone.is_empty();
        for name in names {
            let Some(project) = self.project(&name).await else {
                continue;
            };
            let Ok(trunk) = project.run(|repo| repo.trunk()).await else {
                continue;
            };
            let seen = self
                .0
                .agents
                .read()
                .await
                .get(&name)
                .and_then(|read| read.trunk.clone());
            if seen.as_deref() != Some(trunk.as_str()) {
                self.reload(&name, &project, &trunk).await;
                changed = false;
            }
        }
        if changed {
            (self.0.refresh)();
        }
    }

    /// `name`'s project, opened once it exists.
    async fn project(&self, name: &str) -> Option<Project> {
        let mut projects = self.0.projects.lock().await;
        if let Some(project) = projects.get(name) {
            return Some(project.clone());
        }
        let project =
            Project::open(self.0.root.join(name), Identity::default())
                .await
                .ok()?;
        projects.insert(name.to_owned(), project.clone());
        Some(project)
    }

    /// Loads and tests `name` at `trunk`, makes it live if it passes,
    /// tells the agent if it does not, and redraws the catalog.
    pub async fn reload(&self, name: &str, project: &Project, trunk: &str) {
        let tried = try_commit(name, project, trunk).await;
        let mut kept = read_kept(&self.0.kept_path).await;
        let mut agent_kept = kept.get(name).cloned().unwrap_or_default();
        let mut read = self
            .0
            .agents
            .read()
            .await
            .get(name)
            .cloned()
            .unwrap_or_default();
        read.trunk = Some(trunk.to_owned());
        read.purpose = tried.purpose.clone();
        let Some(version) = tried.version else {
            // No `agent.luau` yet: nothing wakes it, and nothing is wrong.
            read.live = None;
            read.error = None;
            self.0.agents.write().await.insert(name.to_owned(), read);
            (self.0.refresh)();
            return;
        };
        match tried.live {
            Some(live) => {
                if !live.declared.description.is_empty() {
                    read.purpose = live.declared.description.clone();
                }
                agent_kept.live = Some(live.commit.clone());
                read.live = Some(live);
                read.error = None;
            }
            None => {
                // The version before stays; after a restart, it is read
                // from the commit kept.
                if read.live.is_none()
                    && let Some(commit) = agent_kept.live.clone()
                    && commit != trunk
                {
                    let earlier = try_commit(name, project, &commit).await;
                    if let (Some(version), Some(live)) =
                        (earlier.version, earlier.live)
                    {
                        add_version(&mut read.versions, version);
                        read.live = Some(live);
                    }
                }
                read.error = read
                    .live
                    .is_none()
                    .then(|| version.error.clone())
                    .flatten();
                if agent_kept.told.as_deref() != Some(trunk) {
                    agent_kept.told = Some(trunk.to_owned());
                    (self.0.failed)(
                        name,
                        failure_event(&version, self.0.clock.now()),
                    );
                }
            }
        }
        add_version(&mut read.versions, version);
        let live = read.live.as_ref().map(|live| live.commit.clone());
        for version in &mut read.versions {
            version.live = live.as_deref() == Some(version.commit.as_str());
        }
        kept.insert(name.to_owned(), agent_kept);
        write_kept(&self.0.kept_path, &kept).await;
        self.0.agents.write().await.insert(name.to_owned(), read);
        (self.0.refresh)();
    }
}

/// What reading one commit of an agent gave.
struct Tried {
    /// `None` when the commit has no `agent.luau`.
    version: Option<Version>,
    /// The version, when it may be live: it loads and its tests pass.
    live: Option<Live>,
    purpose: String,
}

/// Reads `name`'s folder at `commit`, loads it and runs its tests.
async fn try_commit(name: &str, project: &Project, commit: &str) -> Tried {
    let at = commit.to_owned();
    let paths = project
        .run(move |repo| repo.files_under(&at, ""))
        .await
        .unwrap_or_default();
    let at = commit.to_owned();
    let summary = project
        .run(move |repo| repo.description_of(&at))
        .await
        .ok()
        .and_then(|text| text.lines().next().map(str::to_owned))
        .unwrap_or_default();
    let purpose = paths
        .iter()
        .find(|(path, _)| path == "AGENTS.md")
        .map(|(_, bytes)| first_paragraph(&String::from_utf8_lossy(bytes)))
        .unwrap_or_default();
    if !paths.iter().any(|(path, _)| path == AGENT.file) {
        return Tried {
            version: None,
            live: None,
            purpose,
        };
    }
    let mut version = Version {
        commit: commit.to_owned(),
        summary,
        tests: Vec::new(),
        live: false,
        error: None,
    };
    let loaded = match load(name, &paths).await {
        Ok(loaded) => loaded,
        Err(error) => {
            version.error = Some(error);
            return Tried {
                version: Some(version),
                live: None,
                purpose,
            };
        }
    };
    let (files, modules, declared) = loaded;
    version.tests =
        run_tests(name, modules.clone(), &files, CancellationToken::new())
            .await;
    let failed = version.tests.iter().filter(|test| !test.passed).count();
    if failed > 0 {
        version.error = Some(match failed {
            1 => "1 test fails".to_owned(),
            n => format!("{n} tests fail"),
        });
        return Tried {
            version: Some(version),
            live: None,
            purpose,
        };
    }
    Tried {
        version: Some(version),
        live: Some(Live {
            commit: commit.to_owned(),
            declared,
            files,
            modules,
        }),
        purpose,
    }
}

/// Builds `name`'s folder from `paths` and reads what it declares; its
/// name must be its repository's.
async fn load(
    name: &str,
    paths: &[(String, Vec<u8>)],
) -> Result<(Files, Arc<Modules>, Declared), String> {
    let files = Files::from_paths(paths, AGENT)?;
    let modules = Arc::new(files.modules()?);
    let script = "local tau = require(\"tau\")\n\
                  return tau._describe_agent(require(\"agent\"))\n";
    let value = evaluate(
        name,
        modules.clone(),
        script.into(),
        DESCRIBE_LIMIT,
        CancellationToken::new(),
    )
    .await?;
    let declared: Declared = serde_json::from_value(value)
        .map_err(|error| format!("{}: {error}", AGENT.file))?;
    if declared.name != name {
        return Err(format!(
            "{}: the agent is named `{}`, but its repository is `{name}`",
            AGENT.file, declared.name
        ));
    }
    for trigger in &declared.triggers {
        if matches!(trigger.kind, TriggerKind::Schedule { .. }) {
            Schedule::of(&trigger.kind).map_err(|error| {
                format!("{}: trigger `{}`: {error}", AGENT.file, trigger.name)
            })?;
        }
    }
    Ok((files, modules, declared))
}

/// The event an agent hears when `version` of it failed: what failed,
/// so it can fix it.
fn failure_event(version: &Version, at: i64) -> Event {
    let failing: Vec<_> = version
        .tests
        .iter()
        .filter(|test| !test.passed)
        .map(|test| {
            json!({
                "file": format!("tests/{}.luau", test.file),
                "case": test.name,
                "error": test.error,
            })
        })
        .collect();
    Event {
        trigger: VERSION_TRIGGER.to_owned(),
        kind: Kind::Version,
        id: format!("version:{}", short(&version.commit)),
        at,
        data: json!({
            "commit": version.commit,
            "summary": version.summary,
            "error": version.error,
            "failing": failing,
            "live": "the version before stays live until this is fixed",
        }),
    }
}

/// A commit id as a person reads it.
fn short(commit: &str) -> &str {
    &commit[..commit.len().min(12)]
}

/// `version` as the newest of `versions`, in place of the same commit
/// read before; at most [`VERSIONS`] are kept.
fn add_version(versions: &mut Vec<Version>, version: Version) {
    versions.retain(|known| known.commit != version.commit);
    versions.insert(0, version);
    versions.truncate(VERSIONS);
}

/// The first paragraph of `markdown` that is not a heading, on one line.
fn first_paragraph(markdown: &str) -> String {
    markdown
        .split("\n\n")
        .map(str::trim)
        .find(|block| !block.is_empty() && !block.starts_with('#'))
        .map(|block| block.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// The agents in `root`: each directory whose name can name an agent.
fn agent_dirs(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| tau_agents::valid_name(name))
        .collect();
    names.sort();
    names
}

async fn read_kept(path: &Path) -> BTreeMap<String, Kept> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

async fn write_kept(path: &Path, kept: &BTreeMap<String, Kept>) {
    let (path, text) = (
        path.to_owned(),
        serde_json::to_string_pretty(kept).unwrap_or_default(),
    );
    let written =
        tokio::task::spawn_blocking(move || crate::write_atomic(&path, &text))
            .await;
    if let Ok(Err(error)) = written {
        eprintln!("{}: cannot keep the agents' versions: {error}", crate::NAME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_purpose_is_the_first_paragraph() {
        assert_eq!(
            first_paragraph(
                "# pr-triage\n\nLabels new pull\nrequests.\n\nMore."
            ),
            "Labels new pull requests."
        );
        assert_eq!(first_paragraph("# Only a title\n"), "");
    }

    #[test]
    fn versions_keep_the_newest_few() {
        let version = |commit: &str| Version {
            commit: commit.into(),
            summary: String::new(),
            tests: Vec::new(),
            live: false,
            error: None,
        };
        let mut versions = Vec::new();
        for n in 0..7 {
            add_version(&mut versions, version(&n.to_string()));
        }
        add_version(&mut versions, version("5"));
        let commits: Vec<&str> =
            versions.iter().map(|v| v.commit.as_str()).collect();
        assert_eq!(commits, ["5", "6", "4", "3", "2"]);
    }
}
