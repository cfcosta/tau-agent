//! Where the host keeps its files, and the repositories it lists.

use super::*;

/// What the host needs to start.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// Where sign-ins and keys are kept.
    pub credentials: Credentials,
    /// The model runs use when nothing else is chosen; `None` takes the
    /// provider's first (`Models::fit`).
    pub model: Option<String>,
    /// The run store, usually `$XDG_DATA_HOME/tau/runs.db`.
    pub store: PathBuf,
    /// Where projects live, usually `$XDG_DATA_HOME/tau/repos`.
    pub repos: PathBuf,
    /// The user's model choices, usually `$XDG_CONFIG_HOME/tau/models.json`.
    pub settings: PathBuf,
    /// The repositories tau lists, usually `$XDG_DATA_HOME/tau/repos.json`.
    pub repo_list: PathBuf,
    /// The person's skills, usually `~/.agents/skills` (tau-skills).
    pub skills: PathBuf,
    /// The person's home, where plugins find the other tools' files they
    /// say they read (`Manifest::reads`): `$HOME` in the app. `None`
    /// gives plugins no home, as in tests.
    pub home: Option<PathBuf>,
    /// Who tau's commits are by, and where the jj config they follow
    /// is: the app's is [`Identity::from_environment`], the person's
    /// own; the default reads no config.
    pub identity: Identity,
}

impl HostConfig {
    /// The model runs use when nothing else is chosen, as the person
    /// said it (`--model`); empty for the provider's first
    /// ([`Host::default_model`]).
    pub fn default_model(&self) -> String {
        self.model.clone().unwrap_or_default()
    }

    /// tau's identity under the person's jj config, as this process
    /// finds it ([`Identity::from_environment`]).
    pub fn default_identity() -> Identity {
        Identity::from_environment()
    }

    /// `$XDG_DATA_HOME/tau`, or `~/.local/share/tau`.
    pub fn data_dir() -> PathBuf {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|home| PathBuf::from(home).join(".local/share"))
            })
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tau")
    }

    /// `~/.agents/skills`: where other agents keep the person's skills
    /// too.
    pub fn default_skills() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(tau_skills::SKILLS_DIR)
    }

    pub fn default_store() -> PathBuf {
        Self::data_dir().join("runs.db")
    }

    pub fn default_repos() -> PathBuf {
        Self::data_dir().join("repos")
    }

    pub fn default_repo_list() -> PathBuf {
        Self::data_dir().join("repos.json")
    }

    /// `models.json` in tau's config directory.
    pub fn default_settings() -> PathBuf {
        Credentials::default_dir().dir.join("models.json")
    }

    /// `interface.json` in tau's config directory: the interface's own
    /// settings, such as `reduce_motion`.
    pub fn default_interface_settings() -> PathBuf {
        Credentials::default_dir().dir.join("interface.json")
    }

    /// The directory of a kind of tau's own repository whose root is
    /// `root` (`KindHost::root`): beside the clones', in tau's data
    /// directory.
    pub fn kind_dir(&self, root: &str) -> PathBuf {
        self.repos.parent().unwrap_or(&self.repos).join(root)
    }

    /// The kind of tau's own repository at `path`, of those the plugins
    /// keep (`UiPlugin::repo_kinds`), with the plugin that keeps it.
    pub fn kind_of(
        &self,
        path: &Path,
    ) -> Option<(&'static str, tau_ui_plugin::kinds::KindHost)> {
        use tau_ui_plugin::kinds::KindList;
        crate::hosted::repo_kinds().into_iter().find(|(_, kind)| {
            let dir = self.kind_dir(kind.root);
            match kind.list {
                KindList::One { .. } => path == dir,
                KindList::Each { .. } => path.parent() == Some(dir.as_path()),
            }
        })
    }

    /// Where clones from the forge `id` (`Forge::id`) live, each a
    /// project of its own, under its full name, such as `<owner>/<name>`.
    pub fn forge_dir(&self, id: &str) -> PathBuf {
        self.repos.join(id)
    }

    /// tau's own directory for the local repository `id` (ADR 0043), out
    /// of the person's folder: the main chat's workspace, the runs', and
    /// what plugins keep for it (`RepoCtx::dir`).
    pub fn local_dir(&self, id: &RepoId) -> PathBuf {
        self.repos.join("local").join(&id.0)
    }

    /// tau's directory for data shared by every repository, where each
    /// plugin keeps its own (`HostCx::plugin_dir`).
    pub fn shared_dir(&self) -> PathBuf {
        self.repos.parent().unwrap_or(&self.repos).to_owned()
    }

    /// The project directory for the clone at `path`: its name and a
    /// hash of its full path, so two clones with one name get two
    /// projects. A repository of tau's own, and a clone tau made among
    /// the projects (from the forge, [`Self::forge_dir`]), is its
    /// project.
    pub fn project_dir_of(&self, path: &Path) -> PathBuf {
        // tau's own repositories are projects where they are.
        if self.kind_of(path).is_some()
            || path.starts_with(&self.repos)
            || path.starts_with(canonical(&self.repos))
        {
            return path.to_owned();
        }
        let full = canonical(path);
        self.repos.join(format!(
            "{}-{:08x}",
            dir_name(&full),
            fnv(&full.to_string_lossy())
        ))
    }
}

pub(super) fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned())
}

/// A directory's last component, or `project`.
pub(super) fn dir_name(path: &Path) -> String {
    path.file_name()
        .map_or("project".into(), |name| name.to_string_lossy().into_owned())
}

/// The repositories tau lists, as `repos.json` keeps them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct RepoList {
    pub(super) repos: Vec<Listed>,
    /// The ones the sidebar shows open.
    #[serde(default)]
    pub(super) open: Vec<String>,
    /// Conversations closed: History lists them, the sidebar does not.
    #[serde(default)]
    pub(super) closed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Listed {
    /// Its own id, given as it is first listed: a list saved without one
    /// gets one as it loads, kept from the next save on.
    #[serde(default = "new_id")]
    pub(super) id: RepoId,
    pub(super) name: String,
    pub(super) path: PathBuf,
    /// Removed from tau: not listed, but its runs keep its name.
    #[serde(default)]
    pub(super) hidden: bool,
    /// Cloned from the forge, as its full name (`owner/name`): updates
    /// fetch from there.
    #[serde(default)]
    pub(super) forge: Option<String>,
    /// The id of its main chat, once made; see [`Host::main_of`].
    #[serde(default)]
    pub(super) main: Option<String>,
    /// A repository of tau's own, the plugins repository (ADR 0027) or an
    /// agent's (ADR 0037): made here, not cloned, and listed though it is
    /// not from the forge.
    #[serde(default)]
    pub(super) kind: Option<String>,
    /// A local repository, worked in the person's folder (ADR 0043), at
    /// `path`: what opens it again.
    #[serde(default)]
    pub(super) local: Option<Local>,
}

/// What the host keeps of a local repository (ADR 0043) to open it
/// again: its folder, and tau's directories for it, under
/// [`HostConfig::local_dir`], each named once, as it was added, and
/// never made again from the folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Local {
    /// The person's folder, as inspecting it found it.
    pub(super) folder: PathBuf,
    /// The main chat's workspace's directory.
    pub(super) main_dir: PathBuf,
    /// Where the runs' workspaces are (`local::runs_dir_name`).
    pub(super) runs_dir: PathBuf,
    /// The main chat's workspace's name, as the setup picked it.
    pub(super) main_workspace: String,
    /// Where pushes go, chosen as it was added.
    pub(super) push: PushTo,
    /// The untracked paths in the folder linked into each run's
    /// workspace: none until the person names some.
    pub(super) links: Vec<PathBuf>,
}

/// Where a local repository's pushes go (ADR 0043): to the forge when
/// its `origin` is there and the forge's sign-in could push to it as it
/// was added, else nowhere, which hides Push and pull requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum PushTo {
    Nowhere,
    /// The forge's repository, by its full name (`owner/name`), reached
    /// with the sign-in's token.
    Forge(String),
}

impl RepoList {
    /// The list `path` keeps, without the local checkouts listed before
    /// repositories came from the forge alone: they stay out, open or not.
    /// tau's own repositories and local repositories worked in place
    /// stay.
    pub(super) async fn load(path: &Path) -> Self {
        let list: Self = tokio::fs::read_to_string(path)
            .await
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        list.without_old_checkouts()
    }

    /// The list without the checkouts tau copied before repositories came
    /// from the forge alone: those from the forge, tau's own and local
    /// repositories stay, and only the listed ones are open.
    pub(super) fn without_old_checkouts(mut self) -> Self {
        self.repos.retain(|listed| {
            listed.forge.is_some()
                || listed.kind.is_some()
                || listed.local.is_some()
        });
        let listed: Vec<String> = self
            .repos
            .iter()
            .map(|listed| listed.name.clone())
            .collect();
        self.open.retain(|name| listed.contains(name));
        self
    }

    pub(super) async fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        tokio::fs::write(path, serde_json::to_string_pretty(self)?).await?;
        Ok(())
    }

    /// The id of the repository listed as `name`.
    ///
    /// # Panics
    ///
    /// When none is: callers ask for one they just listed.
    pub(super) fn id_of(&self, name: &str) -> RepoId {
        self.repos
            .iter()
            .find(|listed| listed.name == name)
            .map(|listed| listed.id.clone())
            .expect("the repository is listed")
    }

    /// Lists the clone at `path`, a canonical path, or lists it again if
    /// it was removed, and returns its name: the directory's, made
    /// unique.
    pub(super) fn list(&mut self, path: &Path) -> String {
        self.list_as(path, None)
    }

    /// Lists the repository at `path` as [`Self::list`] does, as one of
    /// tau's own when `kind` says which.
    pub(super) fn list_as(
        &mut self,
        path: &Path,
        kind: Option<String>,
    ) -> String {
        let path = path.to_owned();
        if let Some(listed) =
            self.repos.iter_mut().find(|listed| listed.path == path)
        {
            listed.hidden = false;
            if kind.is_some() {
                listed.kind = kind;
            }
            return listed.name.clone();
        }
        let name = self.free_name(&path);
        self.repos.push(Listed {
            id: new_id(),
            name: name.clone(),
            path,
            hidden: false,
            forge: None,
            main: None,
            kind,
            local: None,
        });
        name
    }

    /// A name for the repository at `path` that none listed has: its
    /// directory's, made unique.
    fn free_name(&self, path: &Path) -> String {
        let base = dir_name(path);
        let taken = |name: &str| self.repos.iter().any(|l| l.name == name);
        (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|name| !taken(name))
            .expect("some suffix is free")
    }

    /// Lists the local repository `local` as `id`, under its folder's
    /// name made unique, and returns the name.
    pub(super) fn list_local(&mut self, id: RepoId, local: Local) -> String {
        let name = self.free_name(&local.folder);
        self.repos.push(Listed {
            id,
            name: name.clone(),
            path: local.folder.clone(),
            hidden: false,
            forge: None,
            main: None,
            kind: None,
            local: Some(local),
        });
        name
    }
}

/// A new repository's id: a UUID, unique on its own, and not made from
/// its path, which may change.
pub(super) fn new_id() -> RepoId {
    RepoId(uuid::Uuid::now_v7().to_string())
}

/// The saved model choices at `path`, or the defaults with `model` for
/// coder when there are none (or the file does not read).
pub(super) async fn load_settings(
    path: &std::path::Path,
    model: &str,
) -> ModelSettings {
    tokio::fs::read_to_string(path)
        .await
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            let mut settings = ModelSettings::default();
            settings
                .set_default("coder", ModelChoice::new(model, Effort::Auto));
            settings
        })
}
