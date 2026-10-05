//! The host's repositories: importing and opening their projects, their
//! main chats, and tagging each run with how it started.

use super::*;

/// Records how a run started: the repository it works on, so history
/// lists the run under it, and its plan, so history shows it.
pub(super) struct RunTag(pub(super) HostRecord);

#[async_trait]
impl Plugin for RunTag {
    fn name(&self) -> &str {
        HOST_RECORD
    }

    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(TagOnce {
            record: self.0.clone(),
            done: false,
        }))
    }
}

pub(super) struct TagOnce {
    pub(super) record: HostRecord,
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
        self.done = ctx.record(&self.record).await.is_ok();
    }
}

/// How a stored run started, if it recorded it.
pub(super) async fn stored_start(
    store: &Store,
    run: &str,
) -> Option<HostRecord> {
    let entries = store.plugin_entries(run, HOST_RECORD).await.ok()?;
    entries
        .iter()
        .find_map(|(_, body)| serde_json::from_str::<HostRecord>(body).ok())
}

/// The repository a stored run recorded, if it did.
pub(super) async fn stored_repo(store: &Store, run: &str) -> Option<String> {
    stored_start(store, run).await.map(|record| record.repo)
}

/// A stable 32-bit FNV-1a hash, for directory names.
pub(super) fn fnv(text: &str) -> u32 {
    text.bytes().fold(0x811c_9dc5, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    })
}

/// The project runs work in, which may still be importing. Anything
/// that needs it awaits [`ProjectSlot::wait`].
pub(super) struct ProjectSlot {
    state: tokio::sync::watch::Sender<ProjectState>,
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
            state: tokio::sync::watch::Sender::new(state),
        })
    }

    pub(super) fn set(&self, state: ProjectState) {
        self.state.send_replace(state);
    }

    pub(super) fn peek(&self) -> ProjectState {
        self.state.borrow().clone()
    }

    /// The project if it is imported now, without waiting.
    pub(super) fn ready(&self) -> Option<Project> {
        match &*self.state.borrow() {
            ProjectState::Ready(project) => Some(project.clone()),
            _ => None,
        }
    }

    /// The project, once the import is over; `None` if it failed.
    pub(super) async fn wait(&self) -> Option<Project> {
        let mut state = self.state.subscribe();
        let state = state
            .wait_for(|state| !matches!(state, ProjectState::Importing))
            .await
            .ok()?;
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
    pub(super) async fn project(&self) -> anyhow::Result<Project> {
        self.project
            .wait()
            .await
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
    let catalog = host.spawn(async move |host| {
        project.wait().await;
        host.catalog().await
    });
    let workspace = workspace.downgrade();
    cx.spawn(async move |cx| {
        let Ok(catalog) = catalog.await else {
            return;
        };
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
        let name = name.to_owned();
        host.spawn(async move |cloner| {
            let repo = cloner.clone_github(&name).await?;
            let main = cloner.main_view(&repo).await?;
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
                    match tau_vcs::ProjectRepo::open_or_import(
                        &source,
                        dir,
                        identity(),
                    ) {
                        Ok(project) => ProjectState::Ready(project.into()),
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
        if let Err(error) = self.setting_up(self.main_of(name)) {
            eprintln!("tau-ui: cannot make {name}'s main chat: {error:#}");
        }
        self
    }

    /// The main chat of the listed repository `repo`, made the first
    /// time it is asked for: a run that starts empty and finished, which
    /// a message resumes. Every other chat in the repository is a fork
    /// of it, and it cannot be closed.
    pub async fn main_of(&self, repo: &str) -> anyhow::Result<RunId> {
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
            && self.store.run(&id).await?.is_some()
        {
            return Ok(RunId(id.into()));
        }
        let id = uuid::Uuid::now_v7().to_string();
        async {
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
                plugin: HOST_RECORD.to_owned(),
                body: serde_json::to_string(&HostRecord {
                    repo: repo.to_owned(),
                    ..HostRecord::default()
                })?,
            };
            self.store
                .append_turn(&id, &[tag], TurnUsage::default())
                .await?;
            self.store.set_title(&id, MAIN_TITLE).await?;
            self.store.finish_run(&id, Status::Done, None, None).await
        }
        .await?;
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
    pub(super) async fn bookmark_of(
        &self,
        run: &RunId,
        project: &Project,
    ) -> anyhow::Result<String> {
        if self.is_main(run) {
            return Ok(project.run(|project| project.trunk_name()).await?);
        }
        Ok(bookmark(run))
    }

    /// Brings a main chat's workspace, `name`, up to trunk, which moves
    /// without it on an update from GitHub: its work in `@` goes onto
    /// trunk's head, so its next commit moves trunk forward, not aside.
    pub(super) async fn catch_up(
        &self,
        project: &Project,
        name: &str,
    ) -> anyhow::Result<()> {
        let known = name.to_owned();
        let (exists, trunk, trunk_name) = project
            .run(move |project| {
                let exists = known == DEFAULT_WORKSPACE
                    || project.workspaces()?.contains(&known);
                anyhow::Ok((exists, project.trunk()?, project.trunk_name()?))
            })
            .await?;
        if !exists {
            return Ok(());
        }
        let vcs =
            tau_vcs::Vcs::open(project.workspace_dir(name), identity()).await?;
        vcs.move_onto(trunk, trunk_name, true).await?;
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
    pub(super) async fn slot_of_run(
        &self,
        run: &RunId,
    ) -> anyhow::Result<RepoSlot> {
        let name = match self.session_of(run).repo {
            Some(name) => Some(name),
            None => stored_repo(&self.store, &run.0).await,
        };
        name.and_then(|name| self.slot(&name)).ok_or_else(|| {
            anyhow::anyhow!("{} works in no listed repository", run.0)
        })
    }

    /// A listed repository's project, waiting for its import.
    pub async fn project_of(&self, repo: &str) -> Option<Project> {
        self.slot(repo)?.project.wait().await
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
    pub(super) async fn list_clone(
        &self,
        dir: &Path,
        full_name: &str,
    ) -> anyhow::Result<Repo> {
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
        repo.main = Some(self.main_of(&name).await?);
        Ok(repo)
    }

    /// Clones `full_name` (`owner/name`) from GitHub with the saved
    /// sign-in, unless it was cloned before, and lists it. Waits for the
    /// clone; the import goes on in the background.
    pub async fn clone_github(&self, full_name: &str) -> anyhow::Result<Repo> {
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
            let (url, into) = (self.github.clone_url(full_name), dir.clone());
            // A clone over the network, through git: it blocks.
            tokio::task::spawn_blocking(move || {
                tau_vcs::clone_bare(&url, Some(&token.token), &into)
            })
            .await??;
        }
        self.list_clone(&dir, full_name).await
    }

    /// Brings new commits from GitHub into a repository's project. New
    /// runs start from the new trunk; runs going on keep their code.
    pub async fn update_repo(
        &self,
        name: &str,
    ) -> anyhow::Result<tau_vcs::Updated> {
        let slot = self
            .slot(name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))?;
        let project = slot.project().await?;
        let full_name = self.github_of(name).ok_or_else(|| {
            anyhow::anyhow!("{name} was not cloned from GitHub")
        })?;
        let token = github::Token::load(&self.config.credentials);
        let url = self.github.clone_url(&full_name);
        Ok(project
            .run(move |project| {
                project.update(tau_vcs::UpdateFrom::Remote {
                    url: &url,
                    token: token.as_ref().map(|token| token.token.as_str()),
                })
            })
            .await?)
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
