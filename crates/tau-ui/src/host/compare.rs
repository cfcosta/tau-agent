//! The compare view's code: what a run and its fork each changed after
//! the fork point, read from the repository.

use tau_ui_plugin::vcs::{ChangeKind, FileDiff};

use super::{landing::last_link, *};

/// See [`Host::branch_code`].
pub(super) async fn branch_code(
    store: Store,
    slot: anyhow::Result<RepoSlot>,
    main: RunId,
    fork: RunId,
) -> anyhow::Result<BranchCode> {
    let repository = slot?.repository().await?;
    let record = store
        .run(&fork.0)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No run {}", fork.0))?;
    let RunKind::Fork { parent, fork_seq } = record.kind else {
        anyhow::bail!("{} is not a fork", fork.0);
    };
    let base = last_link(
        &store.plugin_entries(&parent, WORKSPACE_PLUGIN).await?,
        Some(fork_seq),
    )
    .ok_or_else(|| anyhow::anyhow!("The fork point has no commit"))?;
    let head = |run: &RunId| {
        let store = store.clone();
        let run = run.0.to_string();
        let base = base.clone();
        async move {
            let entries = store.plugin_entries(&run, WORKSPACE_PLUGIN).await?;
            anyhow::Ok(last_link(&entries, None).unwrap_or(base))
        }
    };
    let main_head = head(&main).await?;
    let fork_head = head(&fork).await?;
    // Each at the commit its change has now.
    let [base, main_head, fork_head]: [Link; 3] = repository
        .current(vec![base, main_head, fork_head])
        .await?
        .try_into()
        .map_err(|_| anyhow::anyhow!("Three links went in"))?;
    let (base, main_head, fork_head) =
        (base.commit_id, main_head.commit_id, fork_head.commit_id);
    let stats = |files: Vec<FileDiff>| -> Vec<FileStat> {
        files.iter().map(file_stat).collect()
    };
    Ok(BranchCode {
        main: stats(repository.diff(&base, &main_head).await?),
        fork: stats(repository.diff(&base, &fork_head).await?),
        between: repository
            .diff(&main_head, &fork_head)
            .await?
            .iter()
            .map(|file| FileChange {
                stat: file_stat(file),
                lines: tau_ui_kit::diff::parse(&file.text),
            })
            .collect(),
    })
}

pub(super) fn file_stat(file: &FileDiff) -> FileStat {
    FileStat {
        path: file.path.clone(),
        kind: match file.kind {
            ChangeKind::Added => FileKind::Added,
            ChangeKind::Modified => FileKind::Modified,
            ChangeKind::Removed => FileKind::Removed,
        },
        added: file.added,
        removed: file.removed,
    }
}

impl Host {
    /// The code of `main` and its fork `fork`: what each changed after
    /// the fork point, and how the fork's code differs from the run's.
    pub async fn branch_code(
        &self,
        main: &RunId,
        fork: &RunId,
    ) -> anyhow::Result<BranchCode> {
        branch_code(
            self.store.clone(),
            self.slot_of_run(main).await,
            main.clone(),
            fork.clone(),
        )
        .await
    }
}
