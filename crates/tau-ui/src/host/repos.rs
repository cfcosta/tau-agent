//! The host's repositories: importing and opening their projects, their
//! main chats, and tagging each run with its repository.

use super::*;

/// Records the repository a run works on, so history can list the run
/// under it.
pub(super) struct RepoTag(pub(super) String);

#[async_trait]
impl Plugin for RepoTag {
    fn name(&self) -> &str {
        REPO_RECORD
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(TagOnce {
            repo: self.0.clone(),
            done: false,
        }))
    }
}

pub(super) struct TagOnce {
    pub(super) repo: String,
    pub(super) done: bool,
}

#[async_trait]
impl PluginRun for TagOnce {
    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        // At its first turn: the run is stored by then.
        let RunEvent::TurnStart { run, .. } = event else {
            return;
        };
        if self.done || run != &ctx.run {
            return;
        }
        let record = RepoRecord {
            repo: self.repo.clone(),
        };
        self.done = ctx.record(&record).await.is_ok();
    }
}

/// The repository a stored run recorded, if it did.
pub(super) async fn stored_repo(store: &Store, run: &str) -> Option<String> {
    let entries = store.plugin_entries(run, REPO_RECORD).await.ok()?;
    entries.iter().find_map(|(_, body)| {
        serde_json::from_str::<RepoRecord>(body)
            .ok()
            .map(|record| record.repo)
    })
}

/// A stable 32-bit FNV-1a hash, for directory names.
pub(super) fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// The project runs work in, which may still be importing. Anything
/// that needs it waits in [`ProjectSlot::wait`], off the UI thread where
/// it can.
pub(super) struct ProjectSlot {
    pub(super) state: Mutex<ProjectState>,
    pub(super) done: std::sync::Condvar,
}

#[derive(Clone)]
pub(super) enum ProjectState {
    Importing,
    Ready(Project),
    /// The clone could not be made a project, for this reason; runs
    /// cannot start in it.
    Failed(String),
}

impl ProjectSlot {
    pub(super) fn new(state: ProjectState) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
            done: std::sync::Condvar::new(),
        })
    }

    pub(super) fn set(&self, state: ProjectState) {
        *self.state.lock().expect("not poisoned") = state;
        self.done.notify_all();
    }

    pub(super) fn peek(&self) -> ProjectState {
        self.state.lock().expect("not poisoned").clone()
    }

    /// The project, once the import is over; `None` if it failed.
    pub(super) fn wait(&self) -> Option<Project> {
        let mut state = self.state.lock().expect("not poisoned");
        while matches!(*state, ProjectState::Importing) {
            state = self.done.wait(state).expect("not poisoned");
        }
        match &*state {
            ProjectState::Ready(project) => Some(project.clone()),
            _ => None,
        }
    }
}

/// A listed repository: its clone, and the project runs in it work in.
#[derive(Clone)]
pub(super) struct RepoSlot {
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) project: Arc<ProjectSlot>,
}

impl RepoSlot {
    /// The project runs in the repository work in, once imported.
    pub(super) fn project(&self) -> anyhow::Result<Project> {
        self.project
            .wait()
            .ok_or_else(|| match self.project.peek() {
                ProjectState::Failed(why) => {
                    anyhow::anyhow!(
                        "{} could not be imported: {why}",
                        self.name
                    )
                }
                _ => anyhow::anyhow!("{} has no project", self.name),
            })
    }
}

/// Refreshes the workspace's catalog once `slot` is imported, if it is
/// importing.
pub(super) fn refresh_when_imported(
    host: &Arc<Host>,
    slot: &RepoSlot,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    if !matches!(slot.project.peek(), ProjectState::Importing) {
        return;
    }
    let project = slot.project.clone();
    let wait = host.runtime.spawn_blocking(move || project.wait());
    let host = host.clone();
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let _ = wait.await;
        let catalog = host.catalog();
        let _ = workspace
            .update(cx, |ws, cx| ws.apply(HostUpdate::catalog(catalog), cx));
    })
    .detach();
}

/// A new workspace's name: unique, and sorting by when it was made.
/// A name for a new workspace: `slug` (a message's first words), then
/// the time in hex, so it says what it is for and stays unique.
pub(super) fn workspace_name(slug: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{slug}-{millis:x}")
}

/// Clones a GitHub repository on the host's runtime, reporting how it
/// goes to onboarding's list, and adds it to the sidebar once done.
pub(super) fn clone_into_tau(
    host: &Arc<Host>,
    name: &str,
    workspace: &Entity<Workspace>,
    cx: &mut App,
) {
    let report = |state| {
        SetupUpdate::Clone(RepoClone {
            name: name.to_owned(),
            state,
        })
    };
    workspace.update(cx, |ws, cx| {
        ws.apply(
            HostUpdate::Setup(report(CloneState::Cloning {
                share: 0.3,
                detail: "fetching from GitHub".into(),
            })),
            cx,
        )
    });
    let job = {
        let (cloner, name) = (host.clone(), name.to_owned());
        host.runtime.spawn_blocking(move || {
            let repo = cloner.clone_github(&name)?;
            let main = cloner.main_view(&repo)?;
            anyhow::Ok((repo, main))
        })
    };
    let (host, name) = (host.clone(), name.to_owned());
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let cloned = match job.await {
            Ok(result) => result.map_err(|error| format!("{error:#}")),
            Err(error) => Err(error.to_string()),
        };
        let Some(workspace) = workspace.upgrade() else {
            return;
        };
        cx.update(|cx| {
            let state = match cloned {
                Ok((repo, main)) => {
                    if let Some(slot) = host.slot(&repo.name) {
                        refresh_when_imported(&host, &slot, &workspace, cx);
                    }
                    let main = main.map(Box::new);
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::Repo { repo, main }, cx)
                    });
                    CloneState::Ready
                }
                Err(error) => CloneState::Failed(error),
            };
            let update = SetupUpdate::Clone(RepoClone { name, state });
            workspace
                .update(cx, |ws, cx| ws.apply(HostUpdate::Setup(update), cx));
        });
    })
    .detach();
}

impl Host {
    /// Opens or copies a repository's project on a thread of its own.
    pub(super) fn spawn_import(&self, slot: &RepoSlot) -> anyhow::Result<()> {
        let project = slot.project.clone();
        let source = slot.path.to_string_lossy().into_owned();
        let dir = self.config.project_dir_of(&slot.path);
        std::thread::Builder::new()
            .name("tau-import".into())
            .spawn(move || {
                project.set(
                    match Project::open_or_import(&source, dir, identity()) {
                        Ok(project) => ProjectState::Ready(project),
                        Err(error) => {
                            eprintln!(
                                "tau-ui: cannot import {source}: {error:#}"
                            );
                            ProjectState::Failed(format!("{error:#}"))
                        }
                    },
                );
            })?;
        Ok(())
    }

    /// Lists `project` as the repository `name`, in place of any listed
    /// under that name, for a host built elsewhere: runs in it get a
    /// workspace each there, as in a clone from GitHub.
    pub fn with_repo(self, name: &str, project: Project) -> Self {
        let path = project.root().to_owned();
        {
            let mut list = self.list.lock().expect("not poisoned");
            let main = list
                .repos
                .iter()
                .find(|listed| listed.name == name)
                .and_then(|listed| listed.main.clone());
            list.repos.retain(|listed| listed.name != name);
            list.repos.push(Listed {
                name: name.to_owned(),
                path: path.clone(),
                hidden: false,
                github: None,
                main,
            });
        }
        let mut repos = self.repos.lock().expect("not poisoned");
        repos.retain(|slot| slot.name != name);
        repos.push(RepoSlot {
            name: name.to_owned(),
            path,
            project: ProjectSlot::new(ProjectState::Ready(project)),
        });
        drop(repos);
        if let Err(error) = self.main_of(name) {
            eprintln!("tau-ui: cannot make {name}'s main chat: {error:#}");
        }
        self
    }

    /// The main chat of the listed repository `repo`, made the first
    /// time it is asked for: a run that starts empty and finished, which
    /// a message resumes. Every other chat in the repository is a fork
    /// of it, and it cannot be closed.
    pub fn main_of(&self, repo: &str) -> anyhow::Result<RunId> {
        let listed = {
            let list = self.list.lock().expect("not poisoned");
            let listed =
                list.repos
                    .iter()
                    .find(|listed| listed.name == repo)
                    .ok_or_else(|| anyhow::anyhow!("No repository {repo}"))?;
            listed.main.clone()
        };
        if let Some(id) = listed
            && self.runtime.block_on(self.store.run(&id))?.is_some()
        {
            return Ok(RunId(id.into()));
        }
        let id = uuid::Uuid::now_v7().to_string();
        self.runtime.block_on(async {
            self.store
                .create_run(&tau_store::NewRun {
                    id: &id,
                    workflow_id: None,
                    agent: "coder",
                    kind: RunKind::Root,
                    model: &self.config.default_model(),
                    turns: 0,
                })
                .await?;
            // Tagged with its repository, as a run's first turn would.
            let tag = Entry::Plugin {
                plugin: REPO_RECORD.to_owned(),
                body: serde_json::to_string(&RepoRecord {
                    repo: repo.to_owned(),
                })?,
            };
            self.store
                .append_turn(&id, &[tag], TurnUsage::default())
                .await?;
            self.store.set_title(&id, MAIN_TITLE).await?;
            self.store.finish_run(&id, Status::Done, None, None).await
        })?;
        let mut list = self.list.lock().expect("not poisoned");
        if let Some(listed) =
            list.repos.iter_mut().find(|listed| listed.name == repo)
        {
            listed.main = Some(id.clone());
        }
        list.save(&self.config.repo_list)?;
        Ok(RunId(id.into()))
    }

    /// The main chats of the listed repositories.
    pub(super) fn mains(&self) -> Vec<String> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .filter(|listed| !listed.hidden)
            .filter_map(|listed| listed.main.clone())
            .collect()
    }

    /// Whether `run` is a repository's main chat.
    pub(super) fn is_main(&self, run: &RunId) -> bool {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .any(|listed| listed.main.as_deref() == Some(&*run.0))
    }

    /// The bookmark `run`'s commits move: trunk's for a main chat, which
    /// commits on it, else `tau/<run>`.
    pub(super) fn bookmark_of(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<String> {
        if self.is_main(run) {
            return Ok(project.trunk_name()?);
        }
        Ok(bookmark(run))
    }

    /// Brings a main chat's workspace, `name`, up to trunk, which moves
    /// without it on an update from GitHub: its work in `@` goes onto
    /// trunk's head, so its next commit moves trunk forward, not aside.
    pub(super) fn catch_up(&self, project: &Project, name: &str) -> anyhow::Result<()> {
        let exists = name == DEFAULT_WORKSPACE
            || project.workspaces()?.iter().any(|known| known == name);
        if !exists {
            return Ok(());
        }
        let vcs = tau_vcs::Vcs::open(project.workspace_dir(name), identity())?;
        self.runtime.block_on(vcs.move_onto(
            project.trunk()?,
            project.trunk_name()?,
            true,
        ))?;
        Ok(())
    }

    pub(super) fn slot(&self, name: &str) -> Option<RepoSlot> {
        self.repos
            .lock()
            .expect("not poisoned")
            .iter()
            .find(|slot| slot.name == name)
            .cloned()
    }

    /// The repository `run` works in: as this session started it, or as
    /// the store recorded it.
    pub(super) fn slot_of_run(&self, run: &RunId) -> anyhow::Result<RepoSlot> {
        let known = self
            .run_repos
            .lock()
            .expect("not poisoned")
            .get(run)
            .cloned();
        let name = known.or_else(|| {
            self.runtime.block_on(stored_repo(&self.store, &run.0))
        });
        name.and_then(|name| self.slot(&name)).ok_or_else(|| {
            anyhow::anyhow!("{} works in no listed repository", run.0)
        })
    }

    /// A listed repository's project, waiting for its import.
    pub fn project_of(&self, repo: &str) -> Option<Project> {
        self.slot(repo)?.project.wait()
    }

    /// Whether a repository is still being imported.
    pub fn is_importing(&self) -> bool {
        self.repos
            .lock()
            .expect("not poisoned")
            .iter()
            .any(|slot| matches!(slot.project.peek(), ProjectState::Importing))
    }

    /// Lists the clone of `full_name` at `dir` and starts importing it.
    /// Returns it as the sidebar shows it.
    pub(super) fn list_clone(&self, dir: &Path, full_name: &str) -> anyhow::Result<Repo> {
        let name = {
            let mut list = self.list.lock().expect("not poisoned");
            let name = list.list(dir);
            if let Some(listed) =
                list.repos.iter_mut().find(|listed| listed.name == name)
            {
                listed.github = Some(full_name.to_owned());
            }
            list.save(&self.config.repo_list)?;
            name
        };
        if self.slot(&name).is_none() {
            let slot = RepoSlot {
                name: name.clone(),
                path: canonical(dir),
                project: ProjectSlot::new(ProjectState::Importing),
            };
            self.spawn_import(&slot)?;
            self.repos.lock().expect("not poisoned").push(slot);
        }
        let mut repo = Repo::new(&name, canonical(dir).display().to_string());
        repo.main = Some(self.main_of(&name)?);
        Ok(repo)
    }

    /// Clones `full_name` (`owner/name`) from GitHub with the saved
    /// sign-in, unless it was cloned before, and lists it. Blocks for
    /// the clone; the import goes on in the background.
    pub fn clone_github(&self, full_name: &str) -> anyhow::Result<Repo> {
        let token = github::Token::load(&self.config.credentials)
            .ok_or_else(|| anyhow::anyhow!("Sign in to GitHub first"))?;
        let (owner, name) = full_name
            .split_once('/')
            .filter(|(owner, name)| {
                [owner, name].iter().all(|part| {
                    !part.is_empty()
                        && !part.starts_with('.')
                        && !part.contains('/')
                })
            })
            .ok_or_else(|| anyhow::anyhow!("{full_name} is not owner/name"))?;
        let dir = self.config.repos.join("github").join(owner).join(name);
        if !dir.exists() {
            tau_vcs::clone_bare(
                &self.github.clone_url(full_name),
                Some(&token.token),
                &dir,
            )?;
        }
        self.list_clone(&dir, full_name)
    }

    /// Brings new commits from GitHub into a repository's project. New
    /// runs start from the new trunk; runs going on keep their code.
    /// Blocks.
    pub fn update_repo(&self, name: &str) -> anyhow::Result<tau_vcs::Updated> {
        let slot = self
            .slot(name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))?;
        let project = slot.project()?;
        let full_name = self.github_of(name).ok_or_else(|| {
            anyhow::anyhow!("{name} was not cloned from GitHub")
        })?;
        let token = github::Token::load(&self.config.credentials);
        Ok(project.update(tau_vcs::UpdateFrom::Remote {
            url: &self.github.clone_url(&full_name),
            token: token.as_ref().map(|token| token.token.as_str()),
        })?)
    }

    /// The `owner/name` a repository was cloned from, if it came from
    /// GitHub.
    pub(super) fn github_of(&self, name: &str) -> Option<String> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .find(|listed| listed.name == name)
            .and_then(|listed| listed.github.clone())
    }

    /// Stops listing a repository. Its project and runs stay.
    pub fn hide_repo(&self, name: &str) -> anyhow::Result<()> {
        let mut list = self.list.lock().expect("not poisoned");
        for listed in &mut list.repos {
            if listed.name == name {
                listed.hidden = true;
            }
        }
        list.open.retain(|open| open != name);
        list.save(&self.config.repo_list)
    }

    /// Remembers which repositories the sidebar shows open.
    pub fn set_open_repos(&self, open: Vec<String>) -> anyhow::Result<()> {
        let mut list = self.list.lock().expect("not poisoned");
        list.open = open;
        list.save(&self.config.repo_list)
    }
}
