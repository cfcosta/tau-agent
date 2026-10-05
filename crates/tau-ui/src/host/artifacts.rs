//! Explicit artifact maintenance over every persisted run and its original
//! messages. A missing owner, malformed record, or unreadable root aborts.

use std::collections::{HashMap, HashSet};

use anyhow::{Context as _, bail};
use serde_json::Value;
use tau_artifacts::{Artifact, Bytes, PruneReport, Quotas};
use tau_store::{RunKind, RunRecord, Store};
use tau_tools_host::artifact_grant::{ArtifactRecord, fold_grants};
use tau_ui_plugin::{HOST_RECORD, HostRecord};
use tau_vcs_host::Project;

use super::Host;

/// Largest reachable message sequence per run. A selected fork contributes
/// its own complete history and each ancestor only through its fork prefix.
fn reachable_prefixes(
    runs: &[RunRecord],
    retained: &HashSet<String>,
) -> anyhow::Result<HashMap<String, i64>> {
    let by_id: HashMap<_, _> =
        runs.iter().map(|run| (run.id.as_str(), run)).collect();
    let mut reachable = HashMap::<String, i64>::new();
    for id in retained {
        let run = by_id
            .get(id.as_str())
            .copied()
            .context("retained run missing from inventory")?;
        let mut path = HashSet::new();
        let mut current = run;
        let mut limit = i64::MAX;
        loop {
            if !path.insert(current.id.as_str()) {
                bail!("cycle in retained run history");
            }
            let entry = reachable.entry(current.id.clone()).or_insert(limit);
            *entry = (*entry).max(limit);
            let (parent, cutoff) = match &current.kind {
                RunKind::Root => break,
                RunKind::Fork { parent, fork_seq } => {
                    if *fork_seq < 0 {
                        bail!("negative fork prefix");
                    }
                    (parent, Some(*fork_seq))
                }
                RunKind::Subagent { parent, fork_seq } => {
                    if fork_seq.is_some_and(|seq| seq < 0) {
                        bail!("negative subagent prefix");
                    }
                    (parent, *fork_seq)
                }
            };
            current = by_id
                .get(parent.as_str())
                .copied()
                .with_context(|| format!("missing retained parent {parent}"))?;
            let Some(cutoff) = cutoff else {
                break;
            };
            limit = limit.min(cutoff);
        }
    }
    Ok(reachable)
}

fn collect_refs(
    value: &Value,
    roots: &mut Vec<Artifact>,
) -> anyhow::Result<()> {
    match value {
        Value::Array(items) => {
            for item in items {
                collect_refs(item, roots)?;
            }
        }
        Value::Object(fields) => {
            if fields.contains_key("id")
                && fields.contains_key("digest")
                && fields.contains_key("size_bytes")
            {
                let artifact: Artifact = serde_json::from_value(value.clone())
                    .context("malformed retained artifact reference")?;
                roots.push(artifact);
            }
            for child in fields.values() {
                collect_refs(child, roots)?;
            }
        }
        _ => {}
    }
    Ok(())
}

async fn retained_roots(
    store: &Store,
    repo: &str,
) -> anyhow::Result<Vec<Artifact>> {
    let runs = store.retained_runs().await?;
    let retained: HashSet<String> =
        runs.iter().map(|run| run.id.clone()).collect();
    let reachable = reachable_prefixes(&runs, &retained)?;
    let mut owners = HashMap::new();
    for run in &runs {
        let records = store.plugin_entries(&run.id, HOST_RECORD).await?;
        let Some((_, body)) = records.first() else {
            bail!("run {} has no repository record", run.id);
        };
        let start: HostRecord =
            serde_json::from_str(body).with_context(|| {
                format!("malformed repository record for {}", run.id)
            })?;
        if start.repo.is_empty() || records.len() != 1 {
            bail!("ambiguous repository for {}", run.id);
        }
        owners.insert(run.id.as_str(), start.repo);
    }
    let mut roots = Vec::new();
    for run in &runs {
        if owners[run.id.as_str()] != repo {
            continue;
        }
        let Some(&cutoff) = reachable.get(&run.id) else {
            continue;
        };
        for (seq, body) in store.retention_entries(&run.id).await? {
            if seq > cutoff {
                continue;
            }
            let value: Value =
                serde_json::from_str(&body).with_context(|| {
                    format!("malformed retained body in {}", run.id)
                })?;
            collect_refs(&value, &mut roots)?;
        }
        if let Some(result) = &run.result
            && let Ok(value) = serde_json::from_str::<Value>(result)
        {
            collect_refs(&value, &mut roots)?;
        }
    }
    // Original scoped grants are authoritative even when no current card or
    // script store shows the reference. The scan includes old fork prefixes.
    let grant_rows =
        store.plugin_entries_everywhere(tau_tools::ui::NAME).await?;
    let mut grants = Vec::new();
    for (stored_run, body) in grant_rows {
        let value: Value = serde_json::from_str(&body).with_context(|| {
            format!("malformed tool record in {stored_run}")
        })?;
        if value.get("kind").and_then(Value::as_str) == Some("artifact_grant") {
            let ArtifactRecord::ArtifactGrant(grant) =
                serde_json::from_value(value.clone())?;
            if grant.owner_run_id != stored_run {
                bail!("artifact grant owner mismatch");
            }
        }
        let owner = owners
            .get(stored_run.as_str())
            .context("grant has no retained owner")?;
        if owner == repo {
            grants.push(value);
        }
    }
    for grant in fold_grants(&grants)
        .map_err(anyhow::Error::msg)?
        .into_values()
    {
        roots.push(serde_json::from_value(serde_json::to_value(
            grant.artifact,
        )?)?);
    }
    Ok(roots)
}

impl Host {
    /// Explicit trusted maintenance. The host inventories the complete store
    /// while holding the publication lock through marking and sweeping.
    pub async fn prune_artifacts(
        &self,
        repo: &str,
    ) -> anyhow::Result<PruneReport> {
        let project = self
            .project_of(repo)
            .await
            .context("repository is unavailable")?;
        let (store, repo) = (self.store.clone(), repo.to_owned());
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            prune(&project, &store, &repo, &runtime)
        })
        .await?
    }
}

/// Prunes `repo`'s artifacts under the publication lock, which it holds
/// across its read of the store: a file lock, so it runs in
/// `spawn_blocking` (ADR 0028).
#[allow(
    clippy::disallowed_methods,
    reason = "runs in spawn_blocking, holding a file lock across its store read (ADR 0028)"
)]
fn prune(
    project: &Project,
    store: &Store,
    repo: &str,
    runtime: &tokio::runtime::Handle,
) -> anyhow::Result<PruneReport> {
    let bytes =
        Bytes::new(project.root().join("artifacts"), Quotas::default())?;
    let lease = bytes.lock_publication()?;
    let roots = runtime.block_on(retained_roots(store, repo))?;
    Ok(bytes.prune_with_roots(roots, &lease)?)
}

#[cfg(test)]
#[allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]
mod tests {
    use std::{
        collections::{BTreeMap, HashSet},
        io::Cursor,
    };

    use hegel::{TestCase, generators as gs};
    use serde_json::json;
    use tau_artifacts::{Encoding, Error};
    use tau_store::{Entry, NewRun, Status, TurnUsage};
    use tokio_util::sync::CancellationToken;

    use super::*;

    fn run(id: String, kind: RunKind) -> RunRecord {
        RunRecord {
            id,
            workflow_id: None,
            agent: "test".into(),
            kind,
            model: "fake".into(),
            status: Status::Done,
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            turns: 0,
            result: None,
            error: None,
            title: None,
            created_at: String::new(),
        }
    }

    // Inventory: a retained fork sees exactly its own messages and each
    // ancestor prefix. The oracle walks an index tree independently of run
    // records. Parent indices are drawn below the child, making valid DAGs;
    // smaller vectors, indices and cutoffs shrink to a short fork chain.
    // CI uses the workspace hegel.toml profile and its deterministic seed.
    #[hegel::test]
    fn fork_prefixes_match_index_tree_oracle(tc: TestCase) {
        let count: usize = tc.draw(gs::integers().min_value(1).max_value(12));
        let mut parents = Vec::<(usize, i64)>::new();
        let mut runs = vec![run("r0".into(), RunKind::Root)];
        for index in 1..count {
            let parent: usize = tc.draw(gs::integers().max_value(index - 1));
            let cutoff: i64 = tc.draw(gs::integers().min_value(0).max_value(8));
            parents.push((parent, cutoff));
            runs.push(run(
                format!("r{index}"),
                RunKind::Fork {
                    parent: format!("r{parent}"),
                    fork_seq: cutoff,
                },
            ));
        }
        let selected: Vec<bool> =
            tc.draw(gs::vecs(gs::booleans()).min_size(count).max_size(count));
        let retained: HashSet<String> = selected
            .iter()
            .enumerate()
            .filter(|(_, on)| **on)
            .map(|(index, _)| format!("r{index}"))
            .collect();
        let actual = reachable_prefixes(&runs, &retained).unwrap();
        let mut expected = BTreeMap::<usize, i64>::new();
        for (index, on) in selected.into_iter().enumerate() {
            if !on {
                continue;
            }
            let mut at = index;
            let mut limit = i64::MAX;
            loop {
                expected
                    .entry(at)
                    .and_modify(|old| *old = (*old).max(limit))
                    .or_insert(limit);
                if at == 0 {
                    break;
                }
                let (parent, cutoff) = parents[at - 1];
                limit = limit.min(cutoff);
                at = parent;
            }
        }
        assert_eq!(actual.len(), expected.len());
        for (index, limit) in expected {
            assert_eq!(actual[&format!("r{index}")], limit);
        }
    }

    #[test]
    fn negative_fork_prefix_is_rejected() {
        let runs = vec![
            run("root".into(), RunKind::Root),
            run(
                "fork".into(),
                RunKind::Fork {
                    parent: "root".into(),
                    fork_seq: -1,
                },
            ),
        ];
        let retained = HashSet::from(["fork".to_owned()]);
        assert!(
            reachable_prefixes(&runs, &retained)
                .unwrap_err()
                .to_string()
                .contains("negative fork prefix")
        );
    }

    async fn create_run(store: &Store, id: &str, kind: RunKind) {
        store
            .create_run(&NewRun {
                id,
                workflow_id: None,
                agent: "test",
                kind,
                model: "fake",
                turns: 0,
            })
            .await
            .unwrap();
        store
            .append_turn(
                id,
                &[Entry::Plugin {
                    plugin: HOST_RECORD.into(),
                    body: json!(HostRecord {
                        repo: "repo".into(),
                        ..HostRecord::default()
                    })
                    .to_string(),
                }],
                TurnUsage::default(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn retained_fork_keeps_parent_grant_and_abandoned_object_is_reclaimed()
     {
        let directory = tempfile::tempdir().unwrap();
        let db = directory.path().join("runs.db");
        let store = tau_store_sqlite::open(&db).await.unwrap();
        create_run(&store, "parent", RunKind::Root).await;
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let retained = bytes
            .publish_reader(
                Cursor::new(b"parent bytes"),
                &CancellationToken::new(),
            )
            .unwrap();
        let abandoned = bytes
            .publish_reader(
                Cursor::new(b"abandoned"),
                &CancellationToken::new(),
            )
            .unwrap();
        store.append_turn("parent", &[Entry::Plugin { plugin: tau_tools::ui::NAME.into(), body: json!({
            "kind":"artifact_grant", "artifact":retained.clone(),
            "owner_run_id":"parent", "source":"bash output", "source_complete":true
        }).to_string() }], TurnUsage::default()).await.unwrap();
        create_run(
            &store,
            "fork",
            RunKind::Fork {
                parent: "parent".into(),
                fork_seq: 1,
            },
        )
        .await;
        // The parent is no longer the selected history run; its scoped grant
        // remains a root for the retained fork and the full grant scan.
        let roots = retained_roots(&store, "repo").await.unwrap();
        drop(store);
        let reopened =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let lease = reopened.lock_publication().unwrap();
        assert_eq!(
            reopened
                .prune_with_roots(roots, &lease)
                .unwrap()
                .removed_objects,
            1
        );
        drop(lease);
        assert_eq!(
            reopened
                .read_range(
                    &retained,
                    0,
                    100,
                    Encoding::Utf8,
                    &CancellationToken::new()
                )
                .unwrap()
                .data,
            "parent bytes"
        );
        assert!(matches!(
            reopened.read_range(
                &abandoned,
                0,
                1,
                Encoding::Base64,
                &CancellationToken::new()
            ),
            Err(Error::MissingArtifact)
        ));
    }

    #[tokio::test]
    async fn malformed_grant_aborts_root_inventory() {
        let store = tau_store_sqlite::memory().await.unwrap();
        create_run(&store, "parent", RunKind::Root).await;
        store
            .append_turn(
                "parent",
                &[Entry::Plugin {
                    plugin: tau_tools::ui::NAME.into(),
                    body: json!({"kind":"artifact_grant","artifact":{}})
                        .to_string(),
                }],
                TurnUsage::default(),
            )
            .await
            .unwrap();
        assert!(retained_roots(&store, "repo").await.is_err());
    }
}
