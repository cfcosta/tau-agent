//! Turns and forks through the real plugin (`docs/reference/vcs.md`,
//! "Runs and turns"): an agent with `RunWorkspace` runs drawn turns of
//! file writes, then a fork at a drawn turn continues from it with
//! `Checkpoint::at`, as the host forks. The model is the files after
//! each turn.

use std::{collections::BTreeMap, path::Path, process::Command};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::agent::{Agent, Checkpoint};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};
use tau_vcs::{
    Identity,
    Link,
    Project,
    RunWorkspace,
    run_workspace::{PLUGIN, bookmark},
};

const PATHS: [&str; 3] = ["a.txt", "b.txt", "dir/c.txt"];
const VALUES: [&str; 4] = ["", "one\n", "two\n", "three\n"];

type Tree = BTreeMap<&'static str, &'static str>;

fn git(dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
}

/// A project whose trunk holds `a.txt`: `one`.
fn project(home: &Path) -> Project {
    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("a.txt"), "one\n").unwrap();
    git(&src, &["add", "a.txt"]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    Project::import(src.to_str().unwrap(), home.join("p"), Identity::default())
        .unwrap()
}

fn agent(llm: ScriptedModel, workspace: &RunWorkspace) -> Agent {
    Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())))
        .plugin(workspace.clone())
}

/// A script: each drawn turn writes its files (or only reads, when it
/// has none), and a last turn answers.
fn script(turns: &[Vec<(&'static str, &'static str)>]) -> ScriptedModel {
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
    llm.turn(|t| t.text("done"))
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

fn at_commit(project: &Project, commit: &str) -> Tree {
    PATHS
        .iter()
        .filter_map(|path| {
            let (bytes, _) = project.file_at(commit, path).unwrap()?;
            let value = VALUES.iter().find(|v| v.as_bytes() == bytes);
            Some((*path, *value.unwrap_or_else(|| panic!("{path}: {bytes:?}"))))
        })
        .collect()
}

fn links(entries: &[(i64, String)]) -> Vec<(i64, Link)> {
    entries
        .iter()
        .filter_map(|(seq, body)| Link::parse(body).map(|link| (*seq, link)))
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

/// Every turn links a commit holding that turn's files; a turn that
/// changed nothing links the commit before it; the bookmark names the
/// newest commit; a fork at any turn starts on exactly that turn's
/// files, and neither run sees the other's edits.
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
        let store = Store::memory().await.unwrap();
        let first =
            RunWorkspace::new(project.clone(), "first", Identity::default())
                .unwrap();
        let outcome = agent(script(&turns), &first)
            .run("write", &store)
            .await
            .unwrap();

        // The model: the files after each turn, the answer's included.
        let mut trees = vec![Tree::from([("a.txt", "one\n")])];
        for writes in turns.iter().chain([&Vec::new()]) {
            let mut tree = trees.last().unwrap().clone();
            tree.extend(writes.iter().copied());
            trees.push(tree);
        }
        let turns_linked =
            links(&store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap());
        assert_eq!(turns_linked.len(), turns.len() + 1, "one link per turn");
        let mut previous = trunk.clone();
        for (n, (_, link)) in turns_linked.iter().enumerate() {
            let (before, after) = (&trees[n], &trees[n + 1]);
            assert_eq!(link.turn as usize, n + 1);
            assert_eq!(link.changed, before != after, "turn {}", n + 1);
            if link.changed {
                tc.event("a turn changes files");
                assert_eq!(
                    project.parent_of(&link.commit_id).unwrap(),
                    Some(previous.clone()),
                    "turn {} is not on the turn before",
                    n + 1
                );
            } else {
                tc.event("a turn changes nothing");
                assert_eq!(link.commit_id, previous, "turn {}", n + 1);
            }
            assert_eq!(&at_commit(&project, &link.commit_id), after);
            previous = link.commit_id.clone();
        }
        assert_eq!(
            project.bookmark(&bookmark(&outcome.run)).unwrap(),
            Some(previous.clone()),
            "the bookmark names the newest commit"
        );
        assert_eq!(files(&first.dir()), *trees.last().unwrap());

        // A fork at a drawn turn.
        let k =
            tc.draw(gs::integers::<usize>().max_value(turns_linked.len() - 1));
        let (seq, link) = turns_linked[k].clone();
        let fork =
            RunWorkspace::new(project.clone(), "fork", Identity::default())
                .unwrap();
        let llm = script(&[Vec::new(), fork_writes.clone()]);
        let forked = agent(llm, &fork)
            .fork(&Checkpoint::at(outcome.run.clone(), seq))
            .start("go on", &store)
            .outcome()
            .await
            .unwrap();
        let fork_links =
            links(&store.plugin_entries(&forked.run.0, PLUGIN).await.unwrap());
        // The fork's first own turn only read: it links the turn it
        // started from, so it started on that turn's commit.
        let own: Vec<&Link> = fork_links
            .iter()
            .map(|(_, link)| link)
            .filter(|l| l.workspace == "fork")
            .collect();
        assert_eq!(own.len(), 3, "{fork_links:?}");
        assert!(!own[0].changed);
        assert_eq!(own[0].commit_id, link.commit_id, "the fork's base");
        let mut want = trees[k + 1].clone();
        want.extend(fork_writes.iter().copied());
        assert_eq!(files(&fork.dir()), want, "the fork's files");
        assert_eq!(
            files(&first.dir()),
            *trees.last().unwrap(),
            "the fork's edits reached the run it came from"
        );
        assert_eq!(
            project.bookmark(&bookmark(&outcome.run)).unwrap(),
            Some(previous),
        );
        assert_eq!(
            project.bookmark(&bookmark(&forked.run)).unwrap(),
            Some(own[2].commit_id.clone())
        );
    });
}
