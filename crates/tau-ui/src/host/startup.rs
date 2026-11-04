//! What the host finishes as it starts, for what the last tau left
//! when it closed: workspaces and heads no open run owns.

use std::collections::BTreeSet;

use tau_ui_plugin::vcs::{Held, Keep, Owner, Standing};
use tau_ui_remote::queue::{SubAgentEnd, Waiting};

use super::{
    landing::Reading,
    queue::{Lane, Record},
    *,
};

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

/// Where a run stands for a sweep (`Repository::sweep`):
///
/// - a run going on in this process, or under one that is, keeps what
///   it has;
/// - a chat that landed lets its workspaces and head go, and one
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
    pub async fn resume_cut_off(&self, run: &RunId) -> anyhow::Result<()> {
        let record = self
            .store
            .run(&run.0)
            .await?
            .ok_or_else(|| anyhow::anyhow!("No run {}", run.0))?;
        if record.status != Status::Interrupted {
            anyhow::bail!("{} was not cut off by tau closing", run.0);
        }
        // Its call is gone: a sub-agent cut off is dropped at start.
        if let RunKind::Subagent { .. } = record.kind {
            anyhow::bail!(
                "A sub-agent cut off by tau closing does not go on; its \
                 caller can spawn another"
            );
        }
        let choice = self.session_of(run).choice.unwrap_or_else(|| {
            ModelChoice::new(record.model.clone(), Effort::Auto)
        });
        self.resume_by_tau(run, CUT_OFF, &choice).await
    }

    /// Finishes what the last tau left as it closed, before updates move
    /// trunk.
    ///
    /// Landings cut off are finished first, then the sweep takes what
    /// no open run owns, and the sub-agents that finished unheard join
    /// their main chat's queue. Returns the landings finished, for their
    /// cards.
    pub async fn recover(&self) -> anyhow::Result<Vec<LandingRecord>> {
        let finished = self.finish_landings().await?;
        self.sweep().await?;
        self.queue_unheard_sub_agents().await?;
        Ok(finished)
    }

    /// Puts in each main chat's queue the sub-agents that finished before
    /// tau closed but whose end it never took (ADR 0026): as if tau had
    /// not closed, they land as the queue drains, and tau's turn on main
    /// reports them. One the queue ever held, one that landed or was
    /// dropped, and one this session follows are left alone.
    pub(super) async fn queue_unheard_sub_agents(&self) -> anyhow::Result<()> {
        for main in self.mains() {
            let main = RunId(main.as_str().into());
            let state = self.repo_state_of(&main).await?;
            let _draining = state.draining.lock().await;
            let heard: HashSet<String> =
                self.store
                    .plugin_entries(&main.0, QUEUE_PLUGIN)
                    .await?
                    .iter()
                    .filter_map(|(_, body)| serde_json::from_str(body).ok())
                    .filter_map(|record: Record| match record {
                        Record::Queued(waiting) => Some(waiting.run),
                        Record::Previewed { run, .. }
                        | Record::Left { run } => Some(run),
                        _ => None,
                    })
                    .collect();
            for record in self.store.subagents(&main.0).await? {
                let child = RunId(record.id.as_str().into());
                let finished =
                    matches!(record.status, Status::Done | Status::Limit);
                if !finished
                    || heard.contains(&record.id)
                    || self.is_sub_agent(&child)
                    || self.ending_of(&child).await?.is_some()
                {
                    continue;
                }
                let title = self.title_of(&child).await?;
                let (changes, conflicts) = self
                    .land_dry(&child, Reading::Forecast)
                    .await
                    .map(|landing| (landing.changes.len(), landing.conflicts))
                    .unwrap_or_default();
                let actions = self
                    .with_lane(&main, |lane: &mut Lane| {
                        lane.queue(Waiting {
                            run: child.0.to_string(),
                            title,
                            changes,
                            conflicts,
                            confirmed: Vec::new(),
                            sub_agent: Some(SubAgentEnd::default()),
                        })
                    })
                    .await?;
                self.perform(&main, actions).await?;
            }
        }
        Ok(())
    }

    /// Sweeps every listed repository's project of the workspaces and
    /// heads no open run owns (`Repository::sweep`): those of runs
    /// gone, landed, dropped, or of sub-agents that failed or were cut
    /// off, whose commits go too. A main chat's checkout, the default
    /// workspace, and the workspaces of open chats stay.
    pub async fn sweep(&self) -> anyhow::Result<()> {
        // No run starts while the sweep reads what the projects hold and
        // who owns it: every repository's, taken in the list's order.
        let slots = self.repos.lock().expect("not poisoned").clone();
        let states: Vec<Arc<RepoState>> = slots
            .iter()
            .map(|slot| self.repo_state(&slot.name))
            .collect();
        let mut _starting = Vec::with_capacity(states.len());
        for state in &states {
            _starting.push(state.starting.lock().await);
        }
        // What the projects hold, before what the store says: a run that
        // starts after is in neither.
        let mut held = Vec::new();
        for slot in slots {
            match slot.repository().await {
                Ok(repository) => {
                    let holds = repository.held().await?;
                    held.push((slot.name.clone(), repository, holds));
                }
                Err(error) => eprintln!("tau-ui: cannot sweep: {error:#}"),
            }
        }
        let runs = runs(&self.store).await?;
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
        for (repo, repository, holds) in held {
            let main = repository.main_workspace();
            let mut owners = self.owners(&repo, main, &runs, &live);
            owners.push(Owner {
                run: String::new(),
                standing: Standing::Kept,
                workspaces: live_workspaces.clone(),
                keep: Keep::Workspace(main.to_owned()),
            });
            // A run of this session may be making a sub-agent's
            // workspace, named after its own, before the store has the
            // sub-agent: those are not swept.
            let workspaces: Vec<String> = holds
                .workspaces
                .into_iter()
                .filter(|name| {
                    !live_workspaces
                        .iter()
                        .any(|live| name.starts_with(&format!("{live}-sub-")))
                })
                .collect();
            let holds = Held {
                workspaces,
                heads: holds.heads,
            };
            repository.sweep(owners, holds).await?;
        }
        Ok(())
    }

    /// The runs of `repo`, whose main chat works in `main`, that may own
    /// its workspaces and heads, and where each stands.
    fn owners(
        &self,
        repo: &str,
        main: &str,
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
                .filter(|name| name != main)
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
                    Keep::Run(RunId(parent.as_str().into()))
                }
                _ => Keep::Workspace(main.to_owned()),
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
