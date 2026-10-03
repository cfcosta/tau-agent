//! What the host finishes as it starts, for what the last tau left
//! when it closed: workspaces and bookmarks no open run owns.

use std::collections::BTreeSet;

use tau_vcs::sweep::{Keep, Owner, Standing};

use super::*;

/// What the store says of every run, for deciding what a sweep keeps.
struct Runs {
    records: Vec<tau_store::RunRecord>,
    /// How each run started, by id: its repository and workspace.
    tags: HashMap<String, HostRecord>,
    /// The workspaces each run's own links name, by id.
    linked: HashMap<String, BTreeSet<String>>,
    endings: HashMap<String, Ending>,
}

async fn runs(store: &Store) -> anyhow::Result<Runs> {
    let mut tags = HashMap::new();
    for (run, body) in store.plugin_entries_everywhere(HOST_RECORD).await? {
        if let Ok(tag) = serde_json::from_str::<HostRecord>(&body) {
            tags.entry(run).or_insert(tag);
        }
    }
    let mut linked: HashMap<String, BTreeSet<String>> = HashMap::new();
    for (run, body) in store.plugin_entries_everywhere(WORKSPACE_PLUGIN).await?
    {
        if let Some(link) = Link::parse(&body) {
            linked.entry(run).or_default().insert(link.workspace);
        }
    }
    Ok(Runs {
        records: store.retained_runs().await?,
        tags,
        linked,
        endings: endings(store).await?,
    })
}

/// Where a run stands for a sweep (see `tau_vcs::sweep`):
///
/// - a run going on in this process, or under one that is, keeps what
///   it has;
/// - a chat that landed lets its workspaces and bookmark go, and one
///   that was dropped its commits too;
/// - any other chat is open, whatever its status, and keeps them: a
///   chat that finished but has not landed, failed, or was cut off can
///   go on;
/// - a sub-agent that failed, was cancelled or was cut off by tau
///   closing is discarded, as its call would have done; one that
///   finished keeps what it has, as a call keeps a workspace it could
///   not finalize for recovery.
fn standing(
    kind: &RunKind,
    status: Status,
    ending: Option<&Ending>,
    live: bool,
) -> Standing {
    if live {
        return Standing::Kept;
    }
    match (ending, kind, status) {
        (Some(Ending::Landed { .. }), _, _) => Standing::Landed,
        (Some(Ending::Dropped), _, _) => Standing::Discarded,
        (
            None,
            RunKind::Subagent { .. },
            Status::Failed
            | Status::Cancelled
            | Status::Running
            | Status::Interrupted,
        ) => Standing::Discarded,
        (None, _, _) => Standing::Kept,
    }
}

/// What tau tells a run it resumes after tau closed in its middle, as
/// the message that starts its next turn.
pub const CUT_OFF: &str = "tau closed while you were working, and cut \
    your last turn off. Your files are as that turn left them, \
    uncommitted changes included, but what it did after its last stored \
    step is not in this conversation. Check where the work stands, then \
    go on.";

impl Host {
    /// Goes on with `run`, which tau closing cut off: in the workspace
    /// it has, with [`CUT_OFF`] as the message, on the model it was on.
    pub fn resume_cut_off(&self, run: &RunId) -> anyhow::Result<()> {
        let record = self
            .runtime
            .block_on(self.store.run(&run.0))?
            .ok_or_else(|| anyhow::anyhow!("No run {}", run.0))?;
        if record.status != Status::Interrupted {
            anyhow::bail!("{} was not cut off by tau closing", run.0);
        }
        // Its call is gone: a sub-agent cut off is dropped at start.
        if let RunKind::Subagent { .. } = record.kind {
            anyhow::bail!(
                "A sub-agent cut off by tau closing does not go on; its \
                 caller can delegate again"
            );
        }
        let choice = self.session_of(run).choice.unwrap_or_else(|| {
            ModelChoice::new(record.model.clone(), Effort::Auto)
        });
        self.resume(run, CUT_OFF, &choice)
    }

    /// Finishes what the last tau left as it closed. Blocks: call it off
    /// the interface's thread, before updates move trunk.
    pub fn recover(&self) -> anyhow::Result<()> {
        self.sweep()
    }

    /// Sweeps every listed repository's project of the workspaces and
    /// bookmarks no open run owns (`tau_vcs::sweep`): those of runs
    /// gone, landed, dropped, or of sub-agents that failed or were cut
    /// off, whose commits go too. A main chat's checkout, the default
    /// workspace, and the workspaces of open chats stay. Blocks: call it
    /// off the interface's thread.
    pub fn sweep(&self) -> anyhow::Result<()> {
        // No run starts while the sweep reads what the projects hold and
        // who owns it.
        let _starting = self.starting.lock().expect("not poisoned");
        let slots = self.repos.lock().expect("not poisoned").clone();
        // What the projects hold, before what the store says: a run that
        // starts after is in neither.
        let mut held = Vec::new();
        for slot in slots {
            match slot.project() {
                Ok(project) => {
                    let workspaces = project.workspaces()?;
                    let bookmarks = project
                        .bookmarks(tau_vcs::sweep::RUN_BOOKMARK_PREFIX)?;
                    held.push((
                        slot.name.clone(),
                        project,
                        workspaces,
                        bookmarks,
                    ));
                }
                Err(error) => eprintln!("tau-ui: cannot sweep: {error:#}"),
            }
        }
        let runs = self.runtime.block_on(runs(&self.store))?;
        // The runs of this session, and the workspaces they work in,
        // which the store may not name yet.
        let (live, live_workspaces): (HashSet<String>, Vec<String>) = {
            let sessions = self.sessions.lock().expect("not poisoned");
            let running = self.runs.lock().expect("not poisoned");
            (
                sessions
                    .keys()
                    .chain(running.keys())
                    .map(|run| run.0.to_string())
                    .collect(),
                sessions
                    .values()
                    .filter_map(|run| run.workspace.clone())
                    .collect(),
            )
        };
        for (repo, project, workspaces, bookmarks) in held {
            let mut owners = self.owners(&repo, &runs, &live);
            owners.push(Owner {
                run: String::new(),
                standing: Standing::Kept,
                workspaces: live_workspaces.clone(),
                keep: Keep::Workspace(DEFAULT_WORKSPACE.to_owned()),
            });
            // A run of this session may be making a sub-agent's
            // workspace, named after its own, before the store has the
            // sub-agent: those are not swept.
            let workspaces: Vec<String> = workspaces
                .into_iter()
                .filter(|name| {
                    !live_workspaces
                        .iter()
                        .any(|live| name.starts_with(&format!("{live}-sub-")))
                })
                .collect();
            let sweep = tau_vcs::sweep::plan(&owners, &workspaces, &bookmarks);
            if !sweep.is_empty() {
                project.sweep(&sweep)?;
            }
        }
        Ok(())
    }

    /// The runs of `repo` that may own its workspaces and bookmarks,
    /// and where each stands.
    fn owners(
        &self,
        repo: &str,
        runs: &Runs,
        live: &HashSet<String>,
    ) -> Vec<Owner> {
        let workspaces_of = |run: &str| -> Vec<String> {
            let tagged = runs
                .tags
                .get(run)
                .and_then(|tag| tag.workspace.as_deref())
                .and_then(|dir| Path::new(dir).file_name())
                .map(|name| name.to_string_lossy().into_owned());
            tagged
                .into_iter()
                .chain(runs.linked.get(run).into_iter().flatten().cloned())
                .filter(|name| name != DEFAULT_WORKSPACE)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        };
        let mut owners = Vec::new();
        for record in &runs.records {
            let Some(tag) = runs.tags.get(&record.id) else {
                continue;
            };
            let id = RunId(record.id.as_str().into());
            if tag.repo != repo || self.is_main(&id) {
                continue;
            }
            let parent = match &record.kind {
                RunKind::Fork { parent, .. }
                | RunKind::Subagent { parent, .. } => Some(parent.clone()),
                RunKind::Root => None,
            };
            // A sub-agent of a run going on is that run's.
            let live = live.contains(&record.id)
                || parent.as_ref().is_some_and(|parent| live.contains(parent));
            let keep = match &parent {
                Some(parent)
                    if !self.is_main(&RunId(parent.as_str().into())) =>
                {
                    Keep::Bookmark(bookmark(&RunId(parent.as_str().into())))
                }
                _ => Keep::Workspace(DEFAULT_WORKSPACE.to_owned()),
            };
            owners.push(Owner {
                run: record.id.clone(),
                standing: standing(
                    &record.kind,
                    record.status,
                    runs.endings.get(&record.id),
                    live,
                ),
                workspaces: workspaces_of(&record.id),
                keep,
            });
        }
        owners
    }
}
