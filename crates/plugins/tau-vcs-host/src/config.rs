//! The jj config a workspace loads, found as the jj CLI finds it
//! (jj-cli 0.45.1's `config.rs` and `cli_util.rs`): jj-lib's defaults
//! and jj-cli's for the keys tau reads, then tau's [`Identity`], then the system's config, the variables jj
//! reads before the person's config, the person's config, the
//! repository's, the workspace's, and the variables that override them
//! all. `--when` scopes are resolved for the workspace last, as jj does
//! once it knows the repository.
//!
//! Where the config is comes from a [`JjEnv`]: the app reads the
//! person's ([`JjEnv::from_environment`]); the default has none, so
//! tests read no file and no variable.
//!
//! Every function here but [`JjEnv::from_environment`] reads files, so
//! runs only inside a job (ADR 0028).
//!
//! Ported from jj-cli, under the Apache License 2.0
//! (`THIRD_PARTY_NOTICES.md`).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use etcetera::BaseStrategy as _;
use jj_lib::{
    config::{
        ConfigFile,
        ConfigGetError,
        ConfigLayer,
        ConfigNamePathBuf,
        ConfigResolutionContext,
        ConfigSource,
        StackedConfig,
        resolve,
    },
    dsl_util::{AliasDeclarationParser, AliasesMap},
    secure_config::SecureConfig,
    settings::UserSettings,
};
use rand::SeedableRng as _;
use rand_chacha::ChaCha20Rng;

use crate::{error::VcsError, vcs::Identity};

/// Where the person's repository configs are, under jj's config
/// directory: a directory per config id.
const REPO_CONFIG_DIR: &str = "repos";
/// Where the person's workspace configs are, the same way.
const WORKSPACE_CONFIG_DIR: &str = "workspaces";

/// jj-cli's defaults for the keys tau reads that jj-lib's defaults lack,
/// copied from jj-cli 0.45.1's `src/config/misc.toml` (`diff.git`,
/// `git` and `snapshot`) and `src/config/revsets.toml` (`revset-aliases`, every
/// one, since an alias the person writes may call any of them).
const CLI_DEFAULTS: &str = r#"
[diff.git]
context = 3

[git]
private-commits = "none()"
sign-on-push = false

[snapshot]
max-new-file-size = "1MiB"
auto-track = "all()"

[revset-aliases]
'trunk()' = '''
latest(
  remote_bookmarks(exact:"main", exact:"origin") |
  remote_bookmarks(exact:"master", exact:"origin") |
  remote_bookmarks(exact:"trunk", exact:"origin") |
  remote_bookmarks(exact:"main", exact:"upstream") |
  remote_bookmarks(exact:"master", exact:"upstream") |
  remote_bookmarks(exact:"trunk", exact:"upstream") |
  root()
)
'''
'builtin_log()' = 'present(@) | ancestors(immutable_heads().., 2) | trunk()'
'builtin_immutable_heads()' = '''
trunk() | tags() | untracked_remote_bookmarks() | untracked_remote_tags()
'''
'immutable_heads()' = 'builtin_immutable_heads()'
'immutable()' = '::(immutable_heads() | root())'
'mutable()' = '~immutable()'
'visible()' = '::visible_heads()'
'hidden()' = '~visible()'
"#;

/// Where jj's config comes from: the person's directories, the
/// computer's and the person's names, and the environment's variables.
/// A value, so a test can make one up; the default is empty, and finds
/// no config at all.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct JjEnv {
    /// The person's config directory (`$XDG_CONFIG_HOME`, or
    /// `~/.config`): jj's own is its `jj/`, with `config.toml`,
    /// `conf.d/`, and the repositories' and workspaces' configs.
    pub config_dir: Option<PathBuf>,
    /// The home directory: where `~/.jjconfig.toml` is, and what `~`
    /// means in a `--when` path.
    pub home_dir: Option<PathBuf>,
    /// The system's config directory, `/etc` on Unix: jj's is its `jj/`.
    pub system_dir: Option<PathBuf>,
    /// The computer's name: `operation.hostname`, and what
    /// `--when.hostnames` matches.
    pub hostname: Option<String>,
    /// The person's login name: `operation.username`.
    pub username: Option<String>,
    /// The environment's variables: `JJ_CONFIG`, `JJ_USER`, `JJ_EMAIL`
    /// and the others jj reads, and what `--when.environments` matches.
    pub vars: HashMap<String, String>,
}

impl std::fmt::Debug for JjEnv {
    /// Leaves the variables' values out: they may hold secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut names: Vec<&String> = self.vars.keys().collect();
        names.sort();
        f.debug_struct("JjEnv")
            .field("config_dir", &self.config_dir)
            .field("home_dir", &self.home_dir)
            .field("system_dir", &self.system_dir)
            .field("hostname", &self.hostname)
            .field("username", &self.username)
            .field("vars", &names)
            .finish()
    }
}

impl JjEnv {
    /// This process's, as the jj CLI reads its own
    /// (`ConfigEnv::from_environment`). Only the app's edge calls it.
    pub fn from_environment() -> Self {
        let config_dir = etcetera::choose_base_strategy()
            .ok()
            .map(|strategy| strategy.config_dir());
        // jj canonicalizes the home directory, as it does the working
        // directory: `$HOME` may be a symbolic link.
        let home_dir = etcetera::home_dir()
            .ok()
            .map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir));
        let vars: HashMap<String, String> = std::env::vars_os()
            .filter_map(|(name, value)| {
                Some((name.into_string().ok()?, value.into_string().ok()?))
            })
            .collect();
        // On Unix `$USER` is what login(1) set: jj's fallback.
        let username = whoami::username()
            .ok()
            .or_else(|| vars.get("USER").cloned());
        Self {
            config_dir,
            home_dir,
            system_dir: cfg!(unix).then(|| PathBuf::from("/etc")),
            hostname: whoami::hostname().ok(),
            username,
            vars,
        }
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.vars.get(name).map(String::as_str)
    }

    /// jj's own config directory.
    fn root_config_dir(&self) -> Option<PathBuf> {
        self.config_dir.as_ref().map(|dir| dir.join("jj"))
    }

    /// The system's config files and directories, in order: none when
    /// `JJ_CONFIG` names the config.
    fn system_paths(&self) -> Vec<PathBuf> {
        match &self.system_dir {
            Some(dir) if self.var("JJ_CONFIG").is_none() => {
                vec![dir.join("jj/config.toml"), dir.join("jj/conf.d")]
            }
            _ => Vec::new(),
        }
    }

    /// The person's config files and directories, in order: what
    /// `JJ_CONFIG` names, or `~/.jjconfig.toml` when it exists, then
    /// `config.toml` and `conf.d/` in jj's config directory.
    fn user_paths(&self) -> Vec<PathBuf> {
        if let Some(paths) = self.var("JJ_CONFIG") {
            return std::env::split_paths(paths)
                .filter(|path| !path.as_os_str().is_empty())
                .collect();
        }
        let home = self
            .home_dir
            .as_ref()
            .map(|home| home.join(".jjconfig.toml"));
        let platform =
            self.root_config_dir().map(|dir| dir.join("config.toml"));
        let dir = self.root_config_dir().map(|dir| dir.join("conf.d"));
        let mut paths = Vec::new();
        if let Some(home) = home
            && (home.exists() || platform.is_none())
        {
            paths.push(home);
        }
        paths.extend(platform);
        paths.extend(dir);
        paths
    }

    /// What the environment sets that the person's config overrides
    /// (jj-cli's `env_base_layer`).
    fn base_layer(&self) -> Result<ConfigLayer, VcsError> {
        let mut layer = ConfigLayer::empty(ConfigSource::EnvBase);
        if let Some(hostname) = &self.hostname {
            layer.set_value("operation.hostname", hostname.as_str())?;
        }
        if let Some(username) = &self.username {
            layer.set_value("operation.username", username.as_str())?;
        }
        if !self.var("NO_COLOR").unwrap_or_default().is_empty() {
            layer.set_value("ui.color", "never")?;
        }
        if let Some(editor) = self.var("VISUAL").or_else(|| self.var("EDITOR"))
        {
            layer.set_value("ui.editor", editor)?;
        }
        Ok(layer)
    }

    /// What the environment sets over every config file (jj-cli's
    /// `env_overrides_layer`).
    fn overrides_layer(&self) -> Result<ConfigLayer, VcsError> {
        let mut layer = ConfigLayer::empty(ConfigSource::EnvOverrides);
        for (var, name) in [
            ("JJ_USER", "user.name"),
            ("JJ_EMAIL", "user.email"),
            ("JJ_TIMESTAMP", "debug.commit-timestamp"),
            ("JJ_OP_TIMESTAMP", "debug.operation-timestamp"),
            ("JJ_OP_HOSTNAME", "operation.hostname"),
            ("JJ_OP_USERNAME", "operation.username"),
            ("JJ_EDITOR", "ui.editor"),
            ("JJ_PAGER", "ui.pager"),
        ] {
            if let Some(value) = self.var(var) {
                layer.set_value(name, value)?;
            }
        }
        if let Some(seed) = self.randomness_seed() {
            layer.set_value("debug.randomness-seed", seed)?;
        }
        Ok(layer)
    }

    fn randomness_seed(&self) -> Option<i64> {
        self.var("JJ_RANDOMNESS_SEED")?.parse().ok()
    }

    /// What picks a new config id, when jj moves a config it finds
    /// beside the repository to the person's directory.
    fn rng(&self) -> ChaCha20Rng {
        let seed = self.var("JJ_RANDOMNESS_SEED").and_then(|s| s.parse().ok());
        match seed {
            Some(seed) => ChaCha20Rng::seed_from_u64(seed),
            None => rand::make_rng(),
        }
    }
}

/// The settings jj loads for the workspace at `workspace_root`, whose
/// repository is at `repo_path` (its `.jj/repo`, or where that file
/// points): what the jj CLI would run with there. Neither needs to
/// exist yet, as for a workspace about to be made. What tau commits
/// with, and what adding a repository shows (who commits, whether it
/// signs). Blocks.
pub fn settings(
    identity: &Identity,
    workspace_root: &Path,
    repo_path: &Path,
) -> Result<UserSettings, VcsError> {
    let env = &identity.env;
    // jj works from canonical paths: the scopes match them, and a
    // repository config remembers its repository by one.
    let workspace_root = canonical(workspace_root);
    let repo_path = canonical(repo_path);
    let mut config = StackedConfig::with_defaults();
    config.add_layer(ConfigLayer::parse(ConfigSource::Default, CLI_DEFAULTS)?);
    // Below everything the person sets: only a fallback.
    let mut tau = ConfigLayer::empty(ConfigSource::Default);
    tau.set_value("user.name", identity.name.as_str())?;
    tau.set_value("user.email", identity.email.as_str())?;
    config.add_layer(tau);
    load_paths(&mut config, ConfigSource::System, &env.system_paths())?;
    config.add_layer(env.base_layer()?);
    load_paths(&mut config, ConfigSource::User, &env.user_paths())?;
    if let Some(root) = env.root_config_dir() {
        let mut rng = env.rng();
        let secure = [
            (
                SecureConfig::new_repo(repo_path.clone()),
                REPO_CONFIG_DIR,
                ConfigSource::Repo,
            ),
            (
                SecureConfig::new_workspace(workspace_root.join(".jj")),
                WORKSPACE_CONFIG_DIR,
                ConfigSource::Workspace,
            ),
        ];
        for (secure, dir, source) in secure {
            // Finds the config `jj config edit` made, as any jj command
            // does; makes none of its own.
            let loaded = secure.maybe_load_config(&mut rng, &root.join(dir))?;
            if let Some(path) = loaded.config_file
                && path.exists()
            {
                config.load_file(source, path)?;
            }
        }
    }
    config.add_layer(env.overrides_layer()?);
    let context = ConfigResolutionContext {
        home_dir: env.home_dir.as_deref(),
        repo_path: Some(&repo_path),
        workspace_path: Some(&workspace_root),
        // tau runs no jj command: a `--when.commands` scope never holds.
        command: None,
        hostname: env.hostname.as_deref().unwrap_or(""),
        environment: &env.vars,
    };
    Ok(UserSettings::from_config(resolve(&config, &context)?)?)
}

/// Where `revset-aliases."trunk()"` is in a repository's config.
const TRUNK_ALIAS: [&str; 2] = ["revset-aliases", "trunk()"];

/// Sets `revset-aliases."trunk()"` to `symbol` (`<branch>@<remote>`)
/// in the config of the repository at `repo_path` (its `.jj/repo`), as
/// `jj git clone` does for the remote's default branch (jj-cli 0.45.1's
/// `write_repo_presets`): in the file `jj config edit --repo` edits,
/// made when there is none, keeping what it already holds. Nothing is
/// written when there is no config directory, as jj writes nothing
/// without a home. Blocks.
pub(crate) fn write_repo_trunk(
    env: &JjEnv,
    repo_path: &Path,
    symbol: &str,
) -> Result<(), VcsError> {
    let Some(root) = env.root_config_dir() else {
        return Ok(());
    };
    let secure = SecureConfig::new_repo(canonical(repo_path));
    let loaded =
        secure.load_config(&mut env.rng(), &root.join(REPO_CONFIG_DIR))?;
    let Some(path) = loaded.config_file else {
        return Ok(());
    };
    let mut file = ConfigFile::load_or_empty(ConfigSource::Repo, path)?;
    file.set_value(TRUNK_ALIAS, symbol)?;
    file.save()?;
    Ok(())
}

/// Loads each of `paths` that exists, a file or a directory of `.toml`
/// files read in name order, as `source`.
fn load_paths(
    config: &mut StackedConfig,
    source: ConfigSource,
    paths: &[PathBuf],
) -> Result<(), VcsError> {
    for path in paths.iter().filter(|path| path.exists()) {
        if path.is_dir() {
            config.load_dir(source, path)?;
        } else {
            config.load_file(source, path)?;
        }
    }
    Ok(())
}

/// `path` canonical, as far as it exists: the rest is joined on as
/// given.
fn canonical(path: &Path) -> PathBuf {
    if let Ok(path) = std::fs::canonicalize(path) {
        return path;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => canonical(parent).join(name),
        _ => path.to_owned(),
    }
}

/// The aliases of the table `table` (`fileset-aliases`,
/// `revset-aliases`) in every layer, lowest first, so a later one
/// redefines an earlier one (jj-cli's `load_aliases_map`). An alias jj
/// cannot read is left out, as jj does after warning.
pub(crate) fn aliases<P>(
    config: &StackedConfig,
    table: &str,
) -> Result<AliasesMap<P, String>, VcsError>
where
    P: AliasDeclarationParser + Default,
{
    let name = ConfigNamePathBuf::from_iter([table]);
    let mut aliases = AliasesMap::new();
    for layer in config.layers() {
        let table = match layer.look_up_table(&name) {
            Ok(Some(table)) => table,
            Ok(None) => continue,
            Err(item) => {
                return Err(ConfigGetError::Type {
                    name: name.to_string(),
                    error: format!(
                        "Expected a table, but is {}",
                        item.type_name()
                    )
                    .into(),
                    source_path: layer.path.clone(),
                }
                .into());
            }
        };
        for (decl, item) in table.iter() {
            let (definition, doc) = match item.as_table_like() {
                Some(table) => (
                    table.get("definition").and_then(|item| item.as_str()),
                    table
                        .get("doc")
                        .and_then(|item| item.as_str())
                        .map(str::to_owned),
                ),
                None => (item.as_str(), None),
            };
            if let Some(definition) = definition {
                aliases.insert(decl, definition, doc).ok();
            }
        }
    }
    Ok(aliases)
}
