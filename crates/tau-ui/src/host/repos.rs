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
    Ready(Arc<dyn Repository>),
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

    /// The repository if it is imported now, without waiting.
    pub(super) fn ready(&self) -> Option<Arc<dyn Repository>> {
        match &*self.state.borrow() {
            ProjectState::Ready(repository) => Some(repository.clone()),
            _ => None,
        }
    }

    /// The repository, once the import is over; `None` if it failed.
    pub(super) async fn wait(&self) -> Option<Arc<dyn Repository>> {
        let mut state = self.state.subscribe();
        let state = state
            .wait_for(|state| !matches!(state, ProjectState::Importing))
            .await
            .ok()?;
        match &*state {
            ProjectState::Ready(repository) => Some(repository.clone()),
            _ => None,
        }
    }
}

/// The jj project behind `repository`: for tests ([`Host::project_of`]).
fn jj_project(repository: &dyn Repository) -> Option<Project> {
    let jj = repository.as_any().downcast_ref::<JjRepository>()?;
    Some(jj.project().clone())
}

/// What one repository serializes (ADR 0028): its own multi-step work,
/// so work in another repository never waits on it.
#[derive(Default)]
pub(super) struct RepoState {
    /// Held while a run starts, a landing rewrites the stack, or a sweep
    /// reads who owns what: a main chat's turn must not start on a
    /// stack a landing is rewriting.
    pub(super) starting: tokio::sync::Mutex<()>,
    /// Held while a main chat's landing queue changes or drains.
    pub(super) draining: tokio::sync::Mutex<()>,
}

/// A listed repository: its clone, and the project runs in it work in.
#[derive(Clone)]
pub(super) struct RepoSlot {
    pub(super) id: RepoId,
    pub(super) name: String,
    /// Where it is fetched from: the forge's URL for a clone from there,
    /// and for a local repository whose pushes go there.
    pub(super) remote: Option<String>,
    pub(super) path: PathBuf,
    /// A local repository, worked in the person's folder (ADR 0043).
    pub(super) local: Option<Local>,
    pub(super) project: Arc<ProjectSlot>,
}

impl RepoSlot {
    /// The repository's project, as the kernel asks it, once imported.
    pub(super) async fn repository(
        &self,
    ) -> anyhow::Result<Arc<dyn Repository>> {
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

/// Why a repository's project cannot be opened: no plugin fills the
/// `tau.vcs` slot.
pub(super) fn no_vcs() -> anyhow::Error {
    anyhow::anyhow!(
        "No plugin fills {}: tau has nothing to open repositories with",
        tau_ui_plugin::vcs::VCS.name()
    )
}

/// Why nothing can be cloned, pushed or opened a pull request on: no
/// plugin fills the `tau.forge` slot.
pub(super) fn no_forge() -> anyhow::Error {
    anyhow::anyhow!(
        "No plugin fills {}: tau has nowhere to clone from, push to or \
         open pull requests on",
        tau_ui_plugin::forge::FORGE.name()
    )
}

/// Why a folder was not added as a local repository
/// ([`Host::add_local`]).
#[derive(Debug)]
pub enum NotAdded {
    /// Inspecting the folder refused it, saying what to add instead: the
    /// VCS plugin's refusal, to downcast.
    Refused(anyhow::Error),
    /// The folder is listed already, as `name`.
    Listed { folder: PathBuf, name: String },
    /// Setting it up or listing it failed: nothing is listed.
    Failed(anyhow::Error),
}

impl std::fmt::Display for NotAdded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(refusal) => write!(f, "{refusal:#}"),
            Self::Listed { folder, name } => {
                write!(f, "{} is listed already, as {name}", folder.display())
            }
            Self::Failed(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for NotAdded {}

/// A name for a new workspace: `slug` (a message's first words), then
/// the time in hex, so it says what it is for and stays unique.
pub(super) fn workspace_name(slug: &str) -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    format!("{slug}-{millis:x}")
}

/// Clones a repository from the forge on the host's runtime, reporting
/// how it goes to onboarding's list, and adds it to the sidebar once
/// done.
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
            id: None,
        })
    };
    let detail = match &host.forge {
        Some(forge) => format!("fetching from {}", forge.name()),
        None => "fetching".into(),
    };
    workspace.update(cx, |ws, cx| {
        ws.apply(
            HostUpdate::Setup(report(CloneState::Cloning {
                share: 0.3,
                detail,
            })),
            cx,
        )
    });
    let job = {
        let name = name.to_owned();
        host.spawn(async move |cloner| {
            let repo = cloner.clone_from_forge(&name).await?;
            let main = cloner.main_view(&repo).await?;
            anyhow::Ok((repo, main))
        })
    };
    let name = name.to_owned();
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
            let (state, id) = match cloned {
                Ok((repo, main)) => {
                    let main = main.map(Box::new);
                    let id = repo.id.clone();
                    workspace.update(cx, |ws, cx| {
                        ws.apply(HostUpdate::Repo { repo, main }, cx)
                    });
                    (CloneState::Ready, Some(id))
                }
                Err(error) => (CloneState::Failed(error), None),
            };
            let update = SetupUpdate::Clone(RepoClone { name, state, id });
            workspace
                .update(cx, |ws, cx| ws.apply(HostUpdate::Setup(update), cx));
        });
    })
    .detach();
}

impl Host {
    /// Opens or copies a repository's project on the host's runtime;
    /// the catalog follows once it is done. A local repository opens
    /// where it was set up, laid out as it was listed.
    pub(super) fn spawn_import(&self, slot: &RepoSlot) {
        let project = slot.project.clone();
        let source = slot.path.to_string_lossy().into_owned();
        let dir = self.config.project_dir_of(&slot.path);
        // tau's own repositories are made here the first time, each with
        // what it starts with; a clone is a project from the start.
        let first_files = if slot.path == self.config.plugins_repo() {
            Some(tau_luau_plugins::repository::first_files())
        } else if self.config.is_agent_path(&slot.path) {
            Some(tau_agents::repository::first_files())
        } else {
            None
        };
        let how = if let Some(local) = &slot.local {
            Open::Local {
                folder: local.folder.clone(),
                main_dir: local.main_dir.clone(),
                runs_dir: local.runs_dir.clone(),
            }
        } else if let Some(first_files) = first_files {
            Open::OrInit(first_files)
        } else if dir == slot.path {
            Open::Existing
        } else {
            Open::OrImport(slot.path.clone())
        };
        let catalog = self.catalog_wanted.clone();
        let vcs = self.vcs.clone();
        self.runtime.spawn(async move {
            let opened = match &vcs {
                Some(vcs) => vcs.open(&dir, how).await,
                None => Err(no_vcs()),
            };
            project.set(match opened {
                Ok(repository) => ProjectState::Ready(repository),
                Err(error) => {
                    eprintln!("tau-ui: cannot import {source}: {error:#}");
                    ProjectState::Failed(format!("{error:#}"))
                }
            });
            catalog.notify_one();
        });
    }

    /// Lists `project` as the repository `name`, in place of any listed
    /// under that name, for a host built elsewhere: runs in it get a
    /// workspace each there, as in a clone from the forge.
    pub fn with_repo(self, name: &str, project: Project) -> Self {
        let path = project.root().to_owned();
        let repository: Arc<dyn Repository> =
            Arc::new(JjRepository::new(project, self.identity()));
        let id = {
            let mut list = self.list.lock().expect("not poisoned");
            let before = list.repos.iter().find(|listed| listed.name == name);
            let main = before.and_then(|listed| listed.main.clone());
            let id = before.map_or_else(new_id, |listed| listed.id.clone());
            list.repos.retain(|listed| listed.name != name);
            list.repos.push(Listed {
                id: id.clone(),
                name: name.to_owned(),
                path: path.clone(),
                hidden: false,
                forge: None,
                main,
                own: None,
                local: None,
            });
            id
        };
        let mut repos = self.repos.lock().expect("not poisoned");
        repos.retain(|slot| slot.name != name);
        repos.push(RepoSlot {
            id,
            name: name.to_owned(),
            remote: None,
            path,
            local: None,
            project: ProjectSlot::new(ProjectState::Ready(repository)),
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
        {
            let mut list = self.list.lock().expect("not poisoned");
            if let Some(listed) =
                list.repos.iter_mut().find(|listed| listed.name == repo)
            {
                listed.main = Some(id.clone());
            }
        }
        self.save_list().await?;
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

    /// What `run`'s commits move: trunk's branch for a main chat, which
    /// commits on it, else the run's own head.
    pub(super) fn onto_of(&self, run: &RunId) -> Onto {
        if self.is_main(run) {
            Onto::Trunk
        } else {
            Onto::Run(run.clone())
        }
    }

    /// The locks of the repository `name`, made the first time.
    pub(super) fn repo_state(&self, name: &str) -> Arc<RepoState> {
        self.repo_states
            .lock()
            .expect("not poisoned")
            .entry(name.to_owned())
            .or_default()
            .clone()
    }

    /// The locks of the repository `run` works in.
    pub(super) async fn repo_state_of(
        &self,
        run: &RunId,
    ) -> anyhow::Result<Arc<RepoState>> {
        Ok(self.repo_state(&self.slot_of_run(run).await?.name))
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

    /// A listed repository's project, waiting for its import: for
    /// tests.
    #[doc(hidden)]
    pub async fn project_of(&self, repo: &str) -> Option<Project> {
        jj_project(self.slot(repo)?.project.wait().await?.as_ref())
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
        let dir = tokio::fs::canonicalize(dir)
            .await
            .unwrap_or_else(|_| dir.to_owned());
        let (name, id) = {
            let mut list = self.list.lock().expect("not poisoned");
            let name = list.list(&dir);
            if let Some(listed) =
                list.repos.iter_mut().find(|listed| listed.name == name)
            {
                listed.forge = Some(full_name.to_owned());
            }
            let id = list.id_of(&name);
            (name, id)
        };
        self.save_list().await?;
        if self.slot(&name).is_none() {
            let slot = RepoSlot {
                id: id.clone(),
                name: name.clone(),
                remote: self
                    .forge
                    .as_ref()
                    .map(|forge| forge.clone_url(full_name)),
                path: dir.clone(),
                local: None,
                project: ProjectSlot::new(ProjectState::Importing),
            };
            self.spawn_import(&slot);
            self.repos.lock().expect("not poisoned").push(slot);
        }
        let mut repo = Repo::new(&name, dir.display().to_string());
        repo.id = id;
        repo.main = Some(self.main_of(&name).await?);
        Ok(repo)
    }

    /// The repositories the forge's sign-in reaches, for setup to pick
    /// from: the most recently pushed first.
    pub async fn forge_repos(
        &self,
    ) -> anyhow::Result<Vec<tau_ui_plugin::forge::ForgeRepo>> {
        let forge = self.forge.as_ref().ok_or_else(no_forge)?;
        forge.repos().await
    }

    /// Clones `full_name` (`owner/name`) from the forge with its
    /// sign-in, unless it was cloned before, and lists it. Waits for the
    /// clone; the import goes on in the background.
    pub async fn clone_from_forge(
        &self,
        full_name: &str,
    ) -> anyhow::Result<Repo> {
        let forge = self.forge.as_ref().ok_or_else(no_forge)?;
        let remote = forge.remote(full_name).await?;
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
        let vcs = self.vcs.as_ref().ok_or_else(no_vcs)?;
        let dir = self.config.forge_dir(forge.id()).join(owner).join(name);
        if !vcs.is_project(&dir) {
            // What is there without a project is a clone that never
            // finished, and tau's own.
            if tokio::fs::try_exists(&dir).await? {
                tokio::fs::remove_dir_all(&dir).await?;
            }
            vcs.clone_repository(remote, &dir).await?;
        }
        self.list_clone(&dir, full_name).await
    }

    /// Adds the folder `folder` on this computer as a local repository,
    /// worked in where it is (ADR 0043): inspects it, sets it up in place
    /// (`Vcs::setup_local`, all or nothing), lists it with where its
    /// pushes go, makes its main chat and moves the main chat's
    /// workspace onto trunk. Returns it as the sidebar shows it.
    ///
    /// `progress` hears of each step of the setup before it starts, and
    /// stops the setup by breaking. It runs while the VCS plugin holds the
    /// folder's lock, on a blocking thread: it must be quick,
    /// as sending on a channel is.
    ///
    /// A folder inspecting refuses comes back as [`NotAdded::Refused`],
    /// with what to add instead; one listed already as
    /// [`NotAdded::Listed`]. One the person removed from the list is
    /// listed again as it was. When adding fails, nothing is listed, and
    /// the setup's failure leaves the folder as it was.
    pub async fn add_local(
        &self,
        folder: &Path,
        progress: impl FnMut(
            tau_ui_plugin::vcs::SetupStep,
        ) -> std::ops::ControlFlow<()>
        + Send
        + 'static,
    ) -> Result<Repo, NotAdded> {
        let vcs = self.vcs.as_ref().ok_or_else(no_vcs);
        let vcs = vcs.map_err(NotAdded::Failed)?;
        let found = vcs.inspect(folder).await.map_err(NotAdded::Refused)?;
        let listed = {
            let list = self.list.lock().expect("not poisoned");
            list.repos
                .iter()
                .find(|listed| {
                    listed
                        .local
                        .as_ref()
                        .is_some_and(|local| local.folder == found.root)
                })
                .map(|listed| (listed.name.clone(), listed.hidden))
        };
        match listed {
            Some((name, false)) => {
                return Err(NotAdded::Listed {
                    folder: found.root,
                    name,
                });
            }
            Some((name, true)) => {
                return self.list_again(&name).await.map_err(NotAdded::Failed);
            }
            None => {}
        }
        let id = new_id();
        let dir = self.config.local_dir(&id);
        let runs = dir.join("runs");
        let runs_dir = runs.join(vcs.runs_dir_name(&runs, &found.root));
        let main_dir = dir.join("main");
        let push = self.push_to(&found).await;
        let folder = found.root;
        let repository = vcs
            .setup_local(
                &folder,
                main_dir.clone(),
                runs_dir.clone(),
                Box::new(progress),
            )
            .await
            .map_err(NotAdded::Failed)?;
        let local = Local {
            folder: folder.clone(),
            main_dir,
            runs_dir,
            main_workspace: repository.main_workspace().to_owned(),
            push,
            links: Vec::new(),
        };
        let name = self
            .list
            .lock()
            .expect("not poisoned")
            .list_local(id.clone(), local.clone());
        let slot = RepoSlot {
            id: id.clone(),
            name: name.clone(),
            remote: self.listed(&name).and_then(|l| self.remote_of(&l)),
            path: folder.clone(),
            local: Some(local),
            project: ProjectSlot::new(ProjectState::Ready(repository.clone())),
        };
        self.repos.lock().expect("not poisoned").push(slot);
        let main = match self.main_of(&name).await {
            Ok(main) => main,
            Err(error) => {
                self.unlist(&name).await;
                return Err(NotAdded::Failed(error));
            }
        };
        // The setup leaves the main chat's workspace on the root commit:
        // its first turn would catch up too, but its files are trunk's
        // from the start.
        let workspace = repository.main_workspace().to_owned();
        if let Err(error) = repository.catch_up(&workspace).await {
            eprintln!(
                "tau-ui: cannot move {name}'s main chat onto trunk: {error:#}"
            );
        }
        self.catalog_changed();
        let mut repo = Repo::new(&name, folder.display().to_string());
        repo.id = id;
        repo.main = Some(main);
        Ok(repo)
    }

    /// Lists the repository `name` again, which the person removed from
    /// the list, as it was. Returns it as the sidebar shows it.
    async fn list_again(&self, name: &str) -> anyhow::Result<Repo> {
        let (id, path) = {
            let mut list = self.list.lock().expect("not poisoned");
            let listed = list
                .repos
                .iter_mut()
                .find(|listed| listed.name == name)
                .ok_or_else(|| anyhow::anyhow!("No repository {name}"))?;
            listed.hidden = false;
            (listed.id.clone(), listed.path.clone())
        };
        self.save_list().await?;
        if self.slot(name).is_none()
            && let Some(listed) = self.listed(name)
        {
            let slot = RepoSlot {
                id: id.clone(),
                name: name.to_owned(),
                remote: self.remote_of(&listed),
                path: path.clone(),
                local: listed.local.clone(),
                project: ProjectSlot::new(ProjectState::Importing),
            };
            self.spawn_import(&slot);
            self.repos.lock().expect("not poisoned").push(slot);
        }
        self.catalog_changed();
        let mut repo = Repo::new(name, path.display().to_string());
        repo.id = id;
        repo.main = Some(self.main_of(name).await?);
        Ok(repo)
    }

    /// The repository listed as `name`, as the list keeps it.
    pub(super) fn listed(&self, name: &str) -> Option<Listed> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .find(|listed| listed.name == name)
            .cloned()
    }

    /// Takes the repository `name` off the list and the host, for an add
    /// that failed after listing it.
    async fn unlist(&self, name: &str) {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .retain(|listed| listed.name != name);
        self.repos
            .lock()
            .expect("not poisoned")
            .retain(|slot| slot.name != name);
        if let Err(error) = self.save_list().await {
            eprintln!("tau-ui: cannot unlist {name}: {error:#}");
        }
    }

    /// Where a local repository's pushes go, chosen as it is added (ADR
    /// 0043): its `origin`'s repository on the forge, SSH or HTTPS, when
    /// the forge's sign-in can push there, else nowhere.
    async fn push_to(&self, found: &LocalFolder) -> PushTo {
        let Some(forge) = &self.forge else {
            return PushTo::Nowhere;
        };
        let Some(full_name) = found
            .origin
            .as_ref()
            .and_then(|origin| forge.repository_of(origin))
        else {
            return PushTo::Nowhere;
        };
        match forge.can_push(&full_name).await {
            Ok(true) => PushTo::Forge(full_name),
            Ok(false) => PushTo::Nowhere,
            Err(error) => {
                eprintln!(
                    "tau-ui: cannot ask {} about {full_name}: {error:#}",
                    forge.name()
                );
                PushTo::Nowhere
            }
        }
    }

    /// Brings new commits from the forge into a repository's project.
    /// New runs start from the new trunk; runs going on keep their code.
    /// Signed out, it fetches without a token.
    pub async fn update_repo(&self, name: &str) -> anyhow::Result<Updated> {
        let slot = self
            .slot(name)
            .ok_or_else(|| anyhow::anyhow!("No repository {name}"))?;
        let repository = slot.repository().await?;
        if let Some(error) = self.local_has_no_remote(name) {
            return Err(error);
        }
        let forge = self.forge.as_ref().ok_or_else(no_forge)?;
        let full_name = self.forge_of(name).ok_or_else(|| {
            anyhow::anyhow!("{name} was not cloned from {}", forge.name())
        })?;
        let remote =
            forge.remote(&full_name).await.unwrap_or_else(|_| Remote {
                url: forge.clone_url(&full_name),
                token: None,
            });
        repository.update(remote).await
    }

    /// Lists tau's own plugins repository (ADR 0027), the first time
    /// tau starts: it is made when it is opened.
    pub(super) async fn list_plugins_repo(&self) {
        {
            let mut list = self.list.lock().expect("not poisoned");
            let path = self.config.plugins_repo();
            if list.repos.iter().any(|listed| {
                listed.own == Some(Own::Plugins) && listed.path == path
            }) {
                return;
            }
            list.repos.push(Listed {
                id: new_id(),
                name: tau_luau_plugins::REPO.to_owned(),
                path,
                hidden: false,
                forge: None,
                main: None,
                own: Some(Own::Plugins),
                local: None,
            });
        }
        if let Err(error) = self.save_list().await {
            eprintln!("tau-ui: cannot list the plugins repository: {error:#}");
        }
    }

    /// Lists every agent's repository under the agents' directory (ADR
    /// 0037) that is not listed yet, and forgets the agents whose
    /// directory is gone.
    pub(super) async fn list_agents(&self) {
        let dir = self.config.agents_dir();
        let mut found = Vec::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let is_dir =
                    entry.file_type().await.is_ok_and(|kind| kind.is_dir());
                let name = entry.file_name().to_string_lossy().into_owned();
                if is_dir && tau_agents::valid_name(&name) {
                    found.push(entry.path());
                }
            }
        }
        found.sort();
        {
            let mut list = self.list.lock().expect("not poisoned");
            list.repos.retain(|listed| {
                listed.own != Some(Own::Agent) || found.contains(&listed.path)
            });
            for path in &found {
                if !list.repos.iter().any(|listed| listed.path == *path) {
                    list.list_as(path, Some(Own::Agent));
                }
            }
        }
        if let Err(error) = self.save_list().await {
            eprintln!("tau-ui: cannot list the agents: {error:#}");
        }
    }

    /// Makes the repository of a new agent, `name` (ADR 0037), lists it,
    /// starts opening its project and makes its main chat, the agent's
    /// conversation. Returns it as the sidebar shows it.
    pub async fn list_agent(&self, name: &str) -> anyhow::Result<Repo> {
        anyhow::ensure!(
            tau_agents::valid_name(name),
            "An agent's name is lower-case letters, digits and dashes, \
             starting with a letter: {name} is not"
        );
        let dir = self.config.agents_dir().join(name);
        anyhow::ensure!(
            !tokio::fs::try_exists(&dir).await?,
            "There is an agent named {name} already"
        );
        tokio::fs::create_dir_all(&dir).await?;
        let (name, id) = {
            let mut list = self.list.lock().expect("not poisoned");
            let name = list.list_as(&dir, Some(Own::Agent));
            let id = list.id_of(&name);
            (name, id)
        };
        self.save_list().await?;
        let slot = RepoSlot {
            id: id.clone(),
            name: name.clone(),
            remote: None,
            path: dir.clone(),
            local: None,
            project: ProjectSlot::new(ProjectState::Importing),
        };
        self.spawn_import(&slot);
        self.repos.lock().expect("not poisoned").push(slot);
        let mut repo = Repo::new(&name, dir.display().to_string());
        repo.id = id;
        repo.main = Some(self.main_of(&name).await?);
        repo.own = Some(Own::Agent);
        Ok(repo)
    }

    /// Where the listed repository `listed` is fetched from: the forge's
    /// URL for a clone from there, and for a local repository whose
    /// pushes go there.
    pub(super) fn remote_of(&self, listed: &Listed) -> Option<String> {
        let forge = self.forge.as_ref()?;
        let pushed =
            listed.local.as_ref().and_then(|local| match &local.push {
                PushTo::Forge(full_name) => Some(full_name),
                PushTo::Nowhere => None,
            });
        listed
            .forge
            .as_ref()
            .or(pushed)
            .map(|full_name| forge.clone_url(full_name))
    }

    /// Whether the listed repository `name` is a local one, worked in
    /// the person's folder (ADR 0043), and where its pushes go.
    pub(super) fn local_push_of(&self, name: &str) -> Option<PushTo> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .find(|listed| listed.name == name)
            .and_then(|listed| listed.local.as_ref())
            .map(|local| local.push.clone())
    }

    /// Why `repo` cannot be pushed, fetched or opened pull requests on
    /// from the forge, when it is a local repository: one whose pushes go
    /// nowhere has no remote; one whose pushes go to the forge waits for
    /// pushing and fetching without touching the person's `origin`
    /// (ADR 0043), which clones' paths would rewrite.
    pub(super) fn local_has_no_remote(
        &self,
        repo: &str,
    ) -> Option<anyhow::Error> {
        Some(match self.local_push_of(repo)? {
            PushTo::Nowhere => anyhow::anyhow!(
                "{repo} has no remote: its pushes go nowhere, so there is \
                 nothing to push to, fetch from or open a pull request on"
            ),
            PushTo::Forge(full_name) => anyhow::anyhow!(
                "{repo} is a local repository: tau does not push it to \
                 {full_name}, fetch it or open pull requests from it yet"
            ),
        })
    }

    /// The full name (`owner/name`) a repository was cloned from, if it
    /// came from the forge.
    pub(super) fn forge_of(&self, name: &str) -> Option<String> {
        self.list
            .lock()
            .expect("not poisoned")
            .repos
            .iter()
            .find(|listed| listed.name == name)
            .and_then(|listed| listed.forge.clone())
    }

    /// Stops listing a repository. Its project and runs stay.
    pub async fn hide_repo(&self, name: &str) -> anyhow::Result<()> {
        {
            let mut list = self.list.lock().expect("not poisoned");
            for listed in &mut list.repos {
                if listed.name == name {
                    listed.hidden = true;
                }
            }
            list.open.retain(|open| open != name);
        }
        self.save_list().await
    }

    /// Remembers which repositories the sidebar shows open.
    pub async fn set_open_repos(
        &self,
        open: Vec<String>,
    ) -> anyhow::Result<()> {
        self.list.lock().expect("not poisoned").open = open;
        self.save_list().await
    }
}
