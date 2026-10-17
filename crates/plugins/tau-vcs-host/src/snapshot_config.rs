//! What a snapshot takes from the jj config, as jj-cli 0.45.1 builds it
//! (`cli_util.rs`'s `snapshot_options_with_start_tracking_matcher`,
//! `auto_tracking_matcher` and `base_ignores`): which new files it starts
//! tracking (`snapshot.auto-track`, a fileset, with the person's
//! `fileset-aliases`), how large a new file may be
//! (`snapshot.max-new-file-size`, 0 for no limit), and what it ignores
//! besides the `.gitignore` files: Git's `core.excludesFile`, or
//! `git/ignore` in the person's config directory, then the Git store's
//! `info/exclude`.
//!
//! Git's config comes from the files the [`JjEnv`] names, found as gix
//! finds them for jj, never from this process's environment.
//!
//! Reads files, so runs only inside a job (ADR 0028).
//!
//! Ported from jj-cli, under the Apache License 2.0
//! (`THIRD_PARTY_NOTICES.md`).

#![allow(
    clippy::disallowed_methods,
    reason = "runs only inside a job in spawn_blocking (ADR 0028)"
)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

use gix::{
    bstr::{BStr, ByteSlice as _},
    config::{
        File as GitConfig,
        KeyRef,
        Source,
        file::{Metadata, includes, init},
        path::interpolate,
    },
};
use jj_lib::{
    config::{ConfigGetError, ConfigNamePathBuf, StackedConfig},
    fileset::{
        self,
        FilesetAliasesMap,
        FilesetDiagnostics,
        FilesetParseContext,
    },
    gitignore::GitIgnoreFile,
    matchers::{Matcher, NothingMatcher},
    repo_path::RepoPath,
    settings::{HumanByteSize, UserSettings},
    store::Store,
    ui_path::RepoPathUiConverter,
    working_copy::SnapshotOptions,
};

use crate::{config::JjEnv, error::VcsError};

/// What one snapshot runs with: [`SnapshotConfig::options`] lends it to
/// jj-lib.
pub(crate) struct SnapshotConfig {
    base_ignores: Arc<GitIgnoreFile>,
    auto_track: Box<dyn Matcher>,
    /// New files over this many bytes stay out. `u64::MAX` when the
    /// config says 0, jj's "no limit".
    max_new_file_size: u64,
}

impl SnapshotConfig {
    /// The snapshot config of the workspace at `workspace_root`, whose
    /// settings are `settings` and whose commits are in `store`.
    pub(crate) fn load(
        env: &JjEnv,
        settings: &UserSettings,
        workspace_root: &Path,
        store: &Store,
    ) -> Result<Self, VcsError> {
        let HumanByteSize(mut max_new_file_size) = settings
            .get_value_with("snapshot.max-new-file-size", TryInto::try_into)?;
        if max_new_file_size == 0 {
            max_new_file_size = u64::MAX;
        }
        Ok(Self {
            base_ignores: base_ignores(env, workspace_root, store)?,
            auto_track: auto_track(settings)?,
            max_new_file_size,
        })
    }

    pub(crate) fn options(&self) -> SnapshotOptions<'_> {
        SnapshotOptions {
            base_ignores: self.base_ignores.clone(),
            progress: None,
            start_tracking_matcher: self.auto_track.as_ref(),
            force_tracking_matcher: &NothingMatcher,
            max_new_file_size: self.max_new_file_size,
        }
    }
}

/// The new files a snapshot starts tracking: `snapshot.auto-track`,
/// parsed as jj parses a fileset from config, with paths from the
/// workspace root.
fn auto_track(settings: &UserSettings) -> Result<Box<dyn Matcher>, VcsError> {
    let pattern = settings.get_string("snapshot.auto-track")?;
    let aliases_map = fileset_aliases(settings.config())?;
    let path_converter = RepoPathUiConverter::Fs {
        cwd: PathBuf::new(),
        base: PathBuf::new(),
    };
    let context = FilesetParseContext {
        aliases_map: &aliases_map,
        path_converter: &path_converter,
    };
    // jj prints the diagnostics as warnings; tau has no one to show
    // them to.
    let mut diagnostics = FilesetDiagnostics::new();
    let expression = fileset::parse(&mut diagnostics, &pattern, &context)
        .map_err(|source| VcsError::AutoTrack {
            pattern: pattern.clone(),
            source: Box::new(source),
        })?;
    Ok(expression.to_matcher())
}

/// The `fileset-aliases` of every layer, lowest first, so a later one
/// redefines an earlier one (jj-cli's `load_aliases_map`). An alias jj
/// cannot read is left out, as jj does after warning.
fn fileset_aliases(
    config: &StackedConfig,
) -> Result<FilesetAliasesMap, VcsError> {
    let name = ConfigNamePathBuf::from_iter(["fileset-aliases"]);
    let mut aliases = FilesetAliasesMap::new();
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

/// What a snapshot ignores before the workspace's own `.gitignore`
/// files: the person's excludes file, then the Git store's
/// `info/exclude`.
fn base_ignores(
    env: &JjEnv,
    workspace_root: &Path,
    store: &Store,
) -> Result<Arc<GitIgnoreFile>, VcsError> {
    let mut ignores = GitIgnoreFile::empty();
    let git_dir = jj_lib::git::get_git_backend(store)
        .ok()
        .map(|backend| backend.git_repo_path().to_owned());
    let config = git_config(env, git_dir.as_deref())?;
    if let Some(path) = excludes_file(env, &config, workspace_root) {
        ignores = ignores.chain_with_file(RepoPath::root(), path)?;
    }
    if let Some(git_dir) = git_dir {
        ignores = ignores
            .chain_with_file(RepoPath::root(), git_dir.join("info/exclude"))?;
    }
    Ok(ignores)
}

/// Git's `core.excludesFile`, from the workspace root when relative, as
/// `git` reads it; `git/ignore` in the person's config directory when
/// unset. None when the setting is not UTF-8, as in jj.
fn excludes_file(
    env: &JjEnv,
    config: &GitConfig,
    workspace_root: &Path,
) -> Option<PathBuf> {
    match config.string("core.excludesFile") {
        Some(value) => {
            let path = value.to_str().ok()?;
            Some(workspace_root.join(expand_home(env, path)))
        }
        None => xdg_config_home(env).map(|dir| dir.join("git").join("ignore")),
    }
}

/// `~/` at the start of `path` as the person's home (jj's
/// `expand_home_path`).
fn expand_home(env: &JjEnv, path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = &env.home_dir
    {
        return home.join(rest);
    }
    PathBuf::from(path)
}

/// `$XDG_CONFIG_HOME` when set and not empty, else `~/.config`, as
/// jj-cli finds Git's config directory.
fn xdg_config_home(env: &JjEnv) -> Option<PathBuf> {
    match env.vars.get("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => env.home_dir.as_ref().map(|home| home.join(".config")),
    }
}

/// Git's config as jj-lib's gix reads it for the repository at
/// `git_dir`, or the person's alone when there is none: the system's,
/// the person's (`$XDG_CONFIG_HOME/git/config`, then `~/.gitconfig`, or
/// `GIT_CONFIG_GLOBAL`), the repository's, includes followed, then the
/// `GIT_CONFIG_COUNT` variables. Files and variables come from `env`.
/// The Git installation's own config is left out, as gix leaves it out
/// for jj.
fn git_config(
    env: &JjEnv,
    git_dir: Option<&Path>,
) -> Result<GitConfig, VcsError> {
    let mut var = |name: &str| -> Option<OsString> {
        if name == "HOME" {
            return env.home_dir.clone().map(PathBuf::into_os_string);
        }
        env.vars.get(name).map(OsString::from)
    };
    // gix finds the system's file under the installation's prefix,
    // `/etc` on Unix: the environment's system directory here.
    let system = Source::System.storage_location(&mut var).and_then(|path| {
        if var("GIT_CONFIG_SYSTEM").is_some() {
            Some(path)
        } else {
            env.system_dir.as_ref().map(|dir| dir.join("gitconfig"))
        }
    });
    let mut paths = vec![(Source::System, system)];
    for source in [Source::Git, Source::User] {
        paths.push((source, source.storage_location(&mut var)));
    }
    paths.push((Source::Local, git_dir.map(|dir| dir.join("config"))));
    let mut metas = paths
        .into_iter()
        .filter_map(|(source, path)| Some((source, path?)))
        .map(|(source, path)| Metadata {
            path: Some(path),
            source,
            level: 0,
            trust: gix::sec::Trust::Full,
        });
    let options = init::Options {
        includes: includes::Options::follow(
            interpolate::Context {
                git_install_dir: None,
                home_dir: env.home_dir.as_deref(),
                home_for_user: Some(interpolate::home_for_user),
            },
            includes::conditional::Context {
                git_dir,
                branch_name: None,
            },
        ),
        ..init::Options::default()
    };
    let git_error =
        |error: &dyn std::fmt::Display| VcsError::GitConfig(error.to_string());
    let mut config = GitConfig::from_paths_metadata_buf(
        &mut metas,
        &mut Vec::new(),
        false,
        init::Options {
            includes: includes::Options::no_follow(),
            ..options
        },
    )
    .map_err(|error| git_error(&error))?
    .unwrap_or_default();
    config
        .resolve_includes(options)
        .map_err(|error| git_error(&error))?;
    if let Some(overrides) = env_overrides(env)? {
        config
            .append(overrides)
            .map_err(|error| git_error(&error))?;
    }
    Ok(config)
}

/// The config `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_<n>` and
/// `GIT_CONFIG_VALUE_<n>` set, as gix's `File::from_env` reads them.
fn env_overrides(env: &JjEnv) -> Result<Option<GitConfig>, VcsError> {
    let Some(count) = env.vars.get("GIT_CONFIG_COUNT") else {
        return Ok(None);
    };
    let bad = |what: String| VcsError::GitConfig(what);
    let count: usize = count.parse().map_err(|_| {
        bad(format!("GIT_CONFIG_COUNT is not a count: {count}"))
    })?;
    if count == 0 {
        return Ok(None);
    }
    let mut config = GitConfig::new(Metadata {
        path: None,
        source: Source::Env,
        level: 0,
        trust: gix::sec::Trust::Full,
    });
    for n in 0..count {
        let key = env
            .vars
            .get(&format!("GIT_CONFIG_KEY_{n}"))
            .ok_or_else(|| bad(format!("GIT_CONFIG_KEY_{n} is not set")))?;
        let value = env
            .vars
            .get(&format!("GIT_CONFIG_VALUE_{n}"))
            .ok_or_else(|| bad(format!("GIT_CONFIG_VALUE_{n} is not set")))?;
        let parsed =
            KeyRef::parse_unvalidated(BStr::new(key)).ok_or_else(|| {
                bad(format!("GIT_CONFIG_KEY_{n} is not a key: {key}"))
            })?;
        config
            .section_mut_or_create_new(
                parsed.section_name,
                parsed.subsection_name,
            )
            .map_err(|error| bad(error.to_string()))?
            .push(parsed.value_name, Some(BStr::new(value)))
            .map_err(|error| bad(error.to_string()))?;
    }
    Ok(Some(config))
}
