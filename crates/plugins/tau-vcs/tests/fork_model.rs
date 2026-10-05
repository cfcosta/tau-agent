//! Turns and forks through the real plugin (`docs/reference/vcs.md`,
//! "Runs and turns"): an agent with `RunWorkspace` runs drawn turns of
//! file writes without committing, then a fork at a drawn turn continues
//! from it with `Checkpoint::at`, as the host forks. The model is the
//! files after each turn. Each turn is a snapshot of `@`, never a commit
//! (ADR 0014); what the run leaves uncommitted is committed at the end,
//! with a message its model writes.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::{collections::BTreeMap, path::Path};

use common::{coder, links, project};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::agent::Checkpoint;
use tau_testing::scripted::ScriptedModel;
use tau_vcs::{
    Identity,
    Link,
    ProjectRepo,
    RunWorkspace,
    run_workspace::{PLUGIN, bookmark},
};

const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
const VALUES: [&str; 4] = ["", "one\n", "two\n", "three\n"];

type Tree = BTreeMap<&'static str, &'static str>;

/// A script: each drawn turn writes its files (or only reads, when it
/// has none), and a last turn answers. A run that would stop with
/// uncommitted files is asked to commit: it answers again without
/// committing, and the model then writes the message tau commits with.
fn script(
    turns: &[Vec<(&'static str, &'static str)>],
    dirty: bool,
) -> ScriptedModel {
    let mut llm = ScriptedModel::new();
    for writes in turns {
        let writes = writes.clone();
        llm = llm.turn(move |mut t| {
            if writes.is_empty() {
                return t.tool_call("read", json!({ "path": "a.txt" }));
            }
            for (path, content) in writes {
                t = t.tool_call(
                    "write",
                    json!({ "path": path, "content": content }),
                );
            }
            t
        });
    }
    let llm = llm.turn(|t| t.text("done"));
    if dirty {
        llm.turn(|t| t.text("still done"))
            .turn(|t| t.text("feat: the drawn writes"))
    } else {
        llm
    }
}

fn files(dir: &Path) -> Tree {
    PATHS
        .iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(dir.join(path)).ok()?;
            let value = VALUES.iter().find(|v| **v == text);
            Some((*path, *value.unwrap_or_else(|| panic!("{path}: {text:?}"))))
        })
        .collect()
}

fn at_commit(project: &ProjectRepo, commit: &str) -> Tree {
    PATHS
        .iter()
        .filter_map(|path| {
            let (bytes, _) = project.file_at(commit, path).unwrap()?;
            let value = VALUES.iter().find(|v| v.as_bytes() == bytes);
            Some((*path, *value.unwrap_or_else(|| panic!("{path}: {bytes:?}"))))
        })
        .collect()
}

#[hegel::composite]
fn turn(tc: &TestCase) -> Vec<(&'static str, &'static str)> {
    tc.draw(
        gs::vecs(gs::tuples!(
            gs::sampled_from(PATHS.to_vec()),
            gs::sampled_from(VALUES.to_vec())
        ))
        .max_size(2),
    )
}

/// Every turn links a snapshot holding that turn's files, on trunk,
/// since nothing is committed during the run; work left uncommitted is
/// one commit at the end, which the bookmark names; a fork at any turn
/// starts on exactly that turn's files, uncommitted, and neither run
/// sees the other's edits.
#[hegel::test(
    test_cases = 20,
    suppress_health_check = [hegel::HealthCheck::TooSlow]
)]
fn a_fork_starts_on_its_turns_files(tc: TestCase) {
    let turns: Vec<Vec<(&'static str, &'static str)>> =
        tc.draw(gs::vecs(turn()).min_size(1).max_size(4));
    let fork_writes = tc.draw(turn());

    let home = tempfile::tempdir().unwrap();
    let project = project(home.path());
    let trunk = project.trunk().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        // The model: the files after each turn, the answer's included.
        let start = Tree::from([("a.txt", "one\n")]);
        let mut trees = vec![start.clone()];
        for writes in turns.iter().chain([&Vec::new()]) {
            let mut tree = trees.last().unwrap().clone();
            tree.extend(writes.iter().copied());
            trees.push(tree);
        }
        let dirty = *trees.last().unwrap() != start;

        let store = tau_store_sqlite::memory().await.unwrap();
        let first = RunWorkspace::new(
            project.clone().into(),
            "first",
            Identity::default(),
        )
        .unwrap();
        let llm = script(&turns, dirty);
        let outcome = coder(llm.clone(), &first, false)
            .run("write", &store)
            .await
            .unwrap();
        llm.assert_exhausted();

        let turns_linked =
            links(&store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap());
        // Asked to commit, the run answered once more: one more turn.
        let expected = turns.len() + 1 + usize::from(dirty);
        assert_eq!(turns_linked.len(), expected, "one link per turn");
        for (n, (_, link)) in turns_linked.iter().enumerate() {
            let at = (n + 1).min(trees.len() - 1);
            let (before, after) = (&trees[at - 1], &trees[at]);
            assert!(link.snapshot, "turn {} links a snapshot", n + 1);
            assert_eq!(link.turn as usize, n + 1);
            assert_eq!(link.changed, before != after, "turn {}", n + 1);
            if link.changed {
                tc.event("a turn changes files");
            } else {
                tc.event("a turn changes nothing");
            }
            assert_eq!(
                project.parent_of(&link.commit_id).unwrap(),
                Some(trunk.clone()),
                "turn {}: nothing was committed under it",
                n + 1
            );
            assert_eq!(&at_commit(&project, &link.commit_id), after);
        }
        let head = project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap();
        if dirty {
            tc.event("the run left work to commit");
            let stack = project.stack(&head).unwrap();
            assert_eq!(stack.len(), 1, "one commit, at the end");
            assert_eq!(stack[0].description.trim(), "feat: the drawn writes");
            assert_eq!(&at_commit(&project, &head), trees.last().unwrap());
        } else {
            assert_eq!(head, trunk, "nothing to commit");
        }
        assert_eq!(files(&first.dir()), *trees.last().unwrap());

        // A fork at a drawn turn.
        let k =
            tc.draw(gs::integers::<usize>().max_value(turns_linked.len() - 1));
        let (seq, link) = turns_linked[k].clone();
        let base = trees[(k + 1).min(trees.len() - 1)].clone();
        let mut want = base.clone();
        want.extend(fork_writes.iter().copied());
        let fork = RunWorkspace::new(
            project.clone().into(),
            "fork",
            Identity::default(),
        )
        .unwrap();
        let fork_llm =
            script(&[Vec::new(), fork_writes.clone()], want != start);
        let forked = coder(fork_llm, &fork, false)
            .fork(&Checkpoint::at(outcome.run.clone(), seq))
            .start("go on", &store)
            .outcome()
            .await
            .unwrap();
        let fork_links =
            links(&store.plugin_entries(&forked.run.0, PLUGIN).await.unwrap());
        // The fork's first own turn only read: its snapshot holds the
        // files of the turn it started from.
        let own: Vec<&Link> = fork_links
            .iter()
            .map(|(_, link)| link)
            .filter(|l| l.workspace == "fork")
            .collect();
        assert!(own.len() >= 3, "{fork_links:?}");
        assert_eq!(
            at_commit(&project, &own[0].commit_id),
            base,
            "the fork's base"
        );
        assert_eq!(
            project.parent_of(&own[0].commit_id).unwrap(),
            project.parent_of(&link.commit_id).unwrap(),
            "the fork is on the snapshot's parent"
        );
        assert_eq!(files(&fork.dir()), want, "the fork's files");
        assert_eq!(
            files(&first.dir()),
            *trees.last().unwrap(),
            "the fork's edits reached the run it came from"
        );
        assert_eq!(
            project.bookmark(&bookmark(&outcome.run)).unwrap(),
            Some(head),
        );
    });
}
