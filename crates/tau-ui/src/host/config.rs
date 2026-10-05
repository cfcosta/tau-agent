//! Where the host keeps its files, and the repositories it lists.

use super::*;

/// What the host needs to start.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The ChatGPT account whose plan runs use at first; see
    /// [`Host::set_account`].
    pub account: AccountId,
    /// Where sign-ins and keys are kept.
    pub credentials: Credentials,
    /// The model runs use when nothing else is chosen; `None` takes
    /// [`DEFAULT_MODEL`].
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
}

impl HostConfig {
    /// The model runs use when nothing else is chosen.
    pub fn default_model(&self) -> String {
        self.model
            .clone()
            .unwrap_or_else(|| DEFAULT_MODEL.to_owned())
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

    /// Where tau's own plugins repository lives (ADR 0027): beside the
    /// projects, under the directory every plugin's data shares.
    pub fn plugins_repo(&self) -> PathBuf {
        self.repos
            .parent()
            .unwrap_or(&self.repos)
            .join(tau_luau_plugins::registry::ROOT)
    }

    /// The project directory for the clone at `path`: its name and a
    /// hash of its full path, so two clones with one name get two
    /// projects. A repository of tau's own is its project.
    pub fn project_dir_of(&self, path: &Path) -> PathBuf {
        if path == self.plugins_repo() {
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct RepoList {
    pub(super) repos: Vec<Listed>,
    /// The ones the sidebar shows open.
    #[serde(default)]
    pub(super) open: Vec<String>,
    /// Conversations closed: History lists them, the sidebar does not.
    #[serde(default)]
    pub(super) closed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Listed {
    pub(super) name: String,
    pub(super) path: PathBuf,
    /// Removed from tau: not listed, but its runs keep its name.
    #[serde(default)]
    pub(super) hidden: bool,
    /// Cloned from GitHub, as `owner/name`: updates fetch from there.
    #[serde(default)]
    pub(super) github: Option<String>,
    /// The id of its main chat, once made; see [`Host::main_of`].
    #[serde(default)]
    pub(super) main: Option<String>,
    /// A repository of tau's own, such as the plugins repository (ADR
    /// 0027): made here, not cloned, and listed though it is not from
    /// GitHub.
    #[serde(default)]
    pub(super) own: bool,
}

impl RepoList {
    /// The list `path` keeps, without the local checkouts listed before
    /// repositories came from GitHub alone: they stay out, open or not.
    /// tau's own repositories stay.
    pub(super) fn load(path: &Path) -> Self {
        let list: Self = std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        list.github_only()
    }

    /// The list without repositories that did not come from GitHub,
    /// tau's own excepted, and with only the listed ones open.
    pub(super) fn github_only(mut self) -> Self {
        self.repos
            .retain(|listed| listed.github.is_some() || listed.own);
        let listed: Vec<String> = self
            .repos
            .iter()
            .map(|listed| listed.name.clone())
            .collect();
        self.open.retain(|name| listed.contains(name));
        self
    }

    pub(super) fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Lists the clone at `path`, or lists it again if it was removed,
    /// and returns its name: the directory's, made unique.
    pub(super) fn list(&mut self, path: &Path) -> String {
        let path = canonical(path);
        if let Some(listed) =
            self.repos.iter_mut().find(|listed| listed.path == path)
        {
            listed.hidden = false;
            return listed.name.clone();
        }
        let base = dir_name(&path);
        let taken = |name: &str| self.repos.iter().any(|l| l.name == name);
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    base.clone()
                } else {
                    format!("{base}-{n}")
                }
            })
            .find(|name| !taken(name))
            .expect("some suffix is free");
        self.repos.push(Listed {
            name: name.clone(),
            path,
            hidden: false,
            github: None,
            main: None,
            own: false,
        });
        name
    }
}

/// The saved model choices at `path`, or the defaults with `model` for
/// coder when there are none (or the file does not read).
pub(super) fn load_settings(
    path: &std::path::Path,
    model: &str,
) -> ModelSettings {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| {
            let mut settings = ModelSettings::default();
            settings
                .set_default("coder", ModelChoice::new(model, Effort::Auto));
            settings
        })
}
