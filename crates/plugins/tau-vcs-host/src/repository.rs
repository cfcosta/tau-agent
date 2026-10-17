//! What fills the `tau.vcs` slot (ADR 0045): the kernel's
//! [`vcs::Vcs`] and [`vcs::Repository`], on jj.

use std::{
    any::Any,
    path::{Path, PathBuf},
    sync::Arc,
};

use tau_agent::tool::RunId;
use tau_ui_plugin::{
    HostCx,
    PluginHost,
    registry::HostFuture,
    vcs::{self, Discard, Held, Land, Landing, Newest, Onto, Open, Owner},
};

use crate::{Head, Identity, Project, Vcs, error::VcsError, sweep};

/// Opens projects on jj, with commits by `identity`: tau-vcs's host
/// state, and what it fills [`vcs::VCS`] with.
#[derive(Debug, Clone)]
pub struct JjVcs {
    pub identity: Identity,
}

/// Made with the identity the host gives, else tau's own.
impl PluginHost for JjVcs {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let identity = cx.services.get::<Identity>().cloned();
        Ok(Self {
            identity: identity.unwrap_or_default(),
        })
    }
}

impl vcs::Vcs for JjVcs {
    fn open<'a>(
        &'a self,
        root: &'a Path,
        how: Open,
    ) -> HostFuture<'a, anyhow::Result<Arc<dyn vcs::Repository>>> {
        Box::pin(async move {
            let (root, identity) = (root.to_owned(), self.identity.clone());
            let project = match how {
                Open::Existing => Project::open(root, identity).await,
                Open::OrInit(files) => {
                    Project::open_or_init(root, identity, files).await
                }
                Open::OrImport(source) => {
                    let source = source.to_string_lossy().into_owned();
                    Project::open_or_import(source, root, identity).await
                }
                // Never `Project::open`: it would take the person's
                // `tau/*` branches for run heads of tau's.
                Open::Local {
                    folder,
                    main_dir,
                    runs_dir,
                } => {
                    let layout =
                        crate::Layout::local(folder, main_dir, runs_dir);
                    Project::open_local(layout, identity).await
                }
            }?;
            let repository = JjRepository::new(project, self.identity.clone());
            Ok(Arc::new(repository) as Arc<dyn vcs::Repository>)
        })
    }
}

/// A [`Project`] as the kernel asks it ([`vcs::Repository`]), opening
/// its workspaces with commits by `identity`.
#[derive(Debug, Clone)]
pub struct JjRepository {
    project: Project,
    identity: Identity,
}

impl JjRepository {
    pub fn new(project: Project, identity: Identity) -> Self {
        Self { project, identity }
    }

    pub fn project(&self) -> &Project {
        &self.project
    }

    /// The workspace `name`, which must exist.
    async fn open(&self, name: &str) -> Result<Vcs, VcsError> {
        Vcs::open(self.project.workspace_dir(name), self.identity.clone()).await
    }

    /// The head `onto` names: trunk's branch, or the run's own.
    async fn head(&self, onto: Onto) -> anyhow::Result<Head> {
        Ok(match onto {
            Onto::Trunk => Head::Branch(
                self.project.run(|project| project.trunk_name()).await?,
            ),
            Onto::Run(run) => Head::Run(run),
        })
    }
}

impl vcs::Repository for JjRepository {
    fn main_workspace(&self) -> &str {
        self.project.main_workspace()
    }

    fn workspace_dir(&self, name: &str) -> PathBuf {
        self.project.workspace_dir(name)
    }

    fn workspaces(&self) -> HostFuture<'_, anyhow::Result<Vec<String>>> {
        Box::pin(async move {
            Ok(self.project.run(|project| project.workspaces()).await?)
        })
    }

    fn catch_up<'a>(
        &'a self,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let known = workspace.to_owned();
            let (exists, trunk, trunk_name) = self
                .project
                .run(move |project| {
                    let exists = known == project.main_workspace()
                        || project.workspaces()?.contains(&known);
                    anyhow::Ok((
                        exists,
                        project.trunk()?,
                        project.trunk_name()?,
                    ))
                })
                .await?;
            if !exists {
                return Ok(());
            }
            self.open(workspace)
                .await?
                .move_onto(trunk, Head::Branch(trunk_name), true)
                .await?;
            Ok(())
        })
    }

    fn conflicts<'a>(
        &'a self,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Vec<String>>> {
        Box::pin(
            async move { Ok(self.open(workspace).await?.conflicts().await?) },
        )
    }

    fn newest<'a>(
        &'a self,
        run: &'a RunId,
        workspace: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Newest>> {
        Box::pin(async move {
            let copy = self.open(workspace).await?.working_copy().await?;
            if !copy.is_committed() {
                return Ok(Newest::Uncommitted);
            }
            // A run that never committed and ended no turn with a
            // snapshot has no head: its working copy's parent is its
            // newest commit.
            let head = Head::Run(run.clone());
            let newest = self
                .project
                .run(move |project| project.head(&head))
                .await?
                .unwrap_or(copy.head);
            Ok(Newest::Commit(newest))
        })
    }

    fn ready_workspace<'a>(
        &'a self,
        name: &'a str,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let name = name.to_owned();
            self.project
                .run(move |project| {
                    project.add_workspace(&name, &project.trunk()?)?;
                    anyhow::Ok(())
                })
                .await
        })
    }

    fn land(&self, land: Land) -> HostFuture<'_, anyhow::Result<Landing>> {
        Box::pin(async move {
            let head = self.head(land.onto).await?;
            let vcs = self.open(&land.workspace).await?;
            Ok(vcs.land(land.child, head, land.confirm).await?)
        })
    }

    fn landed<'a>(
        &'a self,
        child: &'a str,
    ) -> HostFuture<'a, anyhow::Result<Option<Landing>>> {
        Box::pin(async move {
            let child = child.to_owned();
            let landed =
                self.project.run(move |project| project.landed(&child));
            Ok(landed.await?)
        })
    }

    fn forget_workspaces(
        &self,
        names: Vec<String>,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            self.project
                .run(move |project| {
                    names
                        .iter()
                        .try_for_each(|name| project.forget_workspace(name))
                })
                .await?;
            Ok(())
        })
    }

    fn forget_run<'a>(
        &'a self,
        run: &'a RunId,
    ) -> HostFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let head = Head::Run(run.clone());
            let removed =
                self.project.run(move |project| project.remove_head(&head));
            Ok(removed.await?)
        })
    }

    /// A main chat's child keeps what the main chat's working copy stands
    /// on: its commits that an update moved trunk past are its own, not
    /// the child's. Any other keeps what its parent's head has, else
    /// trunk.
    fn discard(&self, discard: Discard) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            let Discard {
                run,
                onto,
                workspace,
            } = discard;
            let child_head = Head::Run(run);
            self.project
                .run(move |project| {
                    let main = onto == Onto::Trunk;
                    let into = match onto {
                        Onto::Trunk => Head::Branch(project.trunk_name()?),
                        Onto::Run(run) => Head::Run(run),
                    };
                    if let Some(head) = project.head(&child_head)? {
                        let stands_on = match project
                            .workspace_head(project.main_workspace())?
                        {
                            Some(wc) if main => project.parent_of(&wc)?,
                            _ => None,
                        };
                        let keep = match stands_on {
                            Some(keep) => keep,
                            None => match project.head(&into)? {
                                Some(keep) => keep,
                                None => project.trunk()?,
                            },
                        };
                        project.abandon_between(&keep, &head)?;
                    }
                    if let Some(name) = workspace {
                        project.forget_workspace(&name)?;
                    }
                    project.remove_head(&child_head)?;
                    anyhow::Ok(())
                })
                .await
        })
    }

    fn held(&self) -> HostFuture<'_, anyhow::Result<Held>> {
        Box::pin(async move {
            self.project
                .run(|project| {
                    anyhow::Ok(Held {
                        workspaces: project.workspaces()?,
                        heads: project.run_heads()?,
                    })
                })
                .await
        })
    }

    fn sweep(
        &self,
        owners: Vec<Owner>,
        held: Held,
    ) -> HostFuture<'_, anyhow::Result<()>> {
        Box::pin(async move {
            let sweep = sweep::plan(&owners, &held.workspaces, &held.heads);
            if !sweep.is_empty() {
                self.project
                    .run(move |project| project.sweep(&sweep))
                    .await?;
            }
            Ok(())
        })
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
