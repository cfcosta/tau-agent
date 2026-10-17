//! Revsets from the jj config, parsed and resolved as jj-cli 0.46.0 does
//! (`cli_util.rs`'s `WorkspaceCommandEnvironment`, `revset_util.rs`):
//! with the person's `revset-aliases` and `fileset-aliases`, their email
//! for `mine()`, the commit time jj would use for dates, and the
//! workspace for `@`. What `git.private-commits` and `immutable_heads()`
//! name. Runs only inside a job (ADR 0028).
//!
//! Ported from jj-cli, under the Apache License 2.0
//! (`THIRD_PARTY_NOTICES.md`).

use std::{borrow::Cow, collections::HashMap, path::Path, sync::Arc};

use chrono::TimeZone as _;
use jj_lib::{
    fileset::FilesetAliasesMap,
    git::REMOTE_NAME_FOR_LOCAL_GIT_REPO,
    ref_name::{RemoteName, WorkspaceName, WorkspaceNameBuf},
    repo::Repo,
    revset::{
        self,
        ResolvedRevsetExpression,
        RevsetAliasesMap,
        RevsetDiagnostics,
        RevsetExpression,
        RevsetExtensions,
        RevsetParseContext,
        RevsetWorkspaceContext,
        SymbolResolver,
        UserRevsetExpression,
    },
    settings::UserSettings,
    time_util::DatePatternContext,
    ui_path::RepoPathUiConverter,
};

use crate::error::VcsError;

/// What a revset from the config is parsed and resolved with.
pub(crate) struct ConfigRevsets {
    aliases: RevsetAliasesMap,
    fileset_aliases: FilesetAliasesMap,
    extensions: RevsetExtensions,
    user_email: String,
    now: DatePatternContext,
    path_converter: RepoPathUiConverter,
    workspace_name: WorkspaceNameBuf,
}

impl ConfigRevsets {
    /// For the workspace `workspace_name` at `workspace_root`, under
    /// `settings`.
    pub(crate) fn new(
        settings: &UserSettings,
        workspace_root: &Path,
        workspace_name: &WorkspaceName,
    ) -> Result<Self, VcsError> {
        // jj's commit time, `debug.commit-timestamp` when set: what
        // `after:` and `before:` dates count from.
        let now = match settings.commit_timestamp() {
            Some(timestamp) => chrono::Local
                .timestamp_millis_opt(timestamp.timestamp.0)
                .single()
                .unwrap_or_else(chrono::Local::now),
            None => chrono::Local::now(),
        };
        Ok(Self {
            aliases: crate::config::aliases(
                settings.config(),
                "revset-aliases",
            )?,
            fileset_aliases: crate::config::aliases(
                settings.config(),
                "fileset-aliases",
            )?,
            extensions: RevsetExtensions::default(),
            user_email: settings.user_email().to_owned(),
            now: now.into(),
            path_converter: RepoPathUiConverter::Fs {
                cwd: workspace_root.to_owned(),
                base: workspace_root.to_owned(),
            },
            workspace_name: workspace_name.to_owned(),
        })
    }

    fn context<'a>(
        &'a self,
        aliases: &'a RevsetAliasesMap,
    ) -> RevsetParseContext<'a> {
        RevsetParseContext {
            aliases_map: aliases,
            local_variables: HashMap::new(),
            user_email: &self.user_email,
            date_pattern_context: self.now,
            // tau's repositories are all Git-backed.
            default_ignored_remote: Some(REMOTE_NAME_FOR_LOCAL_GIT_REPO),
            fileset_aliases_map: &self.fileset_aliases,
            extensions: &self.extensions,
            workspace: Some(RevsetWorkspaceContext {
                path_converter: &self.path_converter,
                workspace_name: &self.workspace_name,
            }),
        }
    }

    /// The revset the config's `name` holds, parsed for `repo`.
    pub(crate) fn setting(
        &self,
        repo: &dyn Repo,
        settings: &UserSettings,
        name: &'static str,
    ) -> Result<(String, Arc<UserRevsetExpression>), VcsError> {
        let text = settings.get_string(name)?;
        let expression = self.parse(repo, name, &text)?;
        Ok((text, expression))
    }

    /// The heads of the immutable commits in `repo`: `immutable_heads()`
    /// and the root (jj-cli's `parse_immutable_heads_expression`).
    pub(crate) fn immutable_heads(
        &self,
        repo: &dyn Repo,
    ) -> Result<Arc<UserRevsetExpression>, VcsError> {
        let name = "revset-aliases.immutable_heads()";
        let (_, _, text, _) = self
            .aliases
            .get_function("immutable_heads", 0)
            .ok_or_else(|| VcsError::ConfigRevset {
            name: name.to_owned(),
            error: "it is not defined".to_owned(),
        })?;
        let text = text.clone();
        Ok(self
            .parse(repo, name, &text)?
            .union(&RevsetExpression::root()))
    }

    fn parse(
        &self,
        repo: &dyn Repo,
        name: &str,
        text: &str,
    ) -> Result<Arc<UserRevsetExpression>, VcsError> {
        let aliases = self.aliases_in(repo);
        // jj prints the diagnostics as warnings; tau has no one to show
        // them to.
        let mut diagnostics = RevsetDiagnostics::new();
        revset::parse(&mut diagnostics, text, &self.context(&aliases)).map_err(
            |error| VcsError::ConfigRevset {
                name: name.to_owned(),
                error: error.to_string(),
            },
        )
    }

    /// The aliases a revset is parsed with in `repo`: the config's, but
    /// with `trunk()` as `root()` when the config's `trunk()` names
    /// something `repo` lacks, as a bookmark it does not have (jj-cli's
    /// `load_from_workspace`, with `try_resolve_trunk_alias`). jj warns
    /// that it fell back, and hints at `jj config edit --repo`; tau has
    /// no one to warn. A `trunk()` that does not parse is left as it
    /// is, for the revset that calls it to fail on, as jj leaves it.
    fn aliases_in(&self, repo: &dyn Repo) -> Cow<'_, RevsetAliasesMap> {
        let Some((_, _, text, _)) = self.aliases.get_function("trunk", 0)
        else {
            return Cow::Borrowed(&self.aliases);
        };
        let context = self.context(&self.aliases);
        let Ok(trunk) =
            revset::parse(&mut RevsetDiagnostics::new(), text, &context)
        else {
            return Cow::Borrowed(&self.aliases);
        };
        let resolver =
            SymbolResolver::new(repo, self.extensions.symbol_resolvers());
        if trunk.resolve_user_expression(repo, &resolver).is_ok() {
            return Cow::Borrowed(&self.aliases);
        }
        let mut aliases = self.aliases.clone();
        aliases
            .insert("trunk()", "root()", None)
            .expect("`trunk()` is a valid declaration");
        Cow::Owned(aliases)
    }

    /// `expression`'s symbols resolved in `repo`.
    pub(crate) fn resolve(
        &self,
        repo: &dyn Repo,
        name: &str,
        expression: &Arc<UserRevsetExpression>,
    ) -> Result<Arc<ResolvedRevsetExpression>, VcsError> {
        let resolver =
            SymbolResolver::new(repo, self.extensions.symbol_resolvers());
        expression
            .resolve_user_expression(repo, &resolver)
            // The expression alone, as jj-cli's `RevsetExpressionEvaluator::
            // resolve` keeps it: the repositories an `at_operation()` brings
            // in are left out.
            .map(|resolved| resolved.into_inner().0)
            .map_err(|error| VcsError::ConfigRevset {
                name: name.to_owned(),
                error: error.to_string(),
            })
    }
}

/// The remote's bookmarks' targets: what it already has, as the last
/// fetch or push left it.
pub(crate) fn remote_heads(
    repo: &dyn Repo,
    remote: &RemoteName,
) -> Vec<jj_lib::backend::CommitId> {
    repo.view()
        .remote_bookmarks(remote)
        .flat_map(|(_, remote_ref)| remote_ref.target.present_adds())
        .cloned()
        .collect()
}
