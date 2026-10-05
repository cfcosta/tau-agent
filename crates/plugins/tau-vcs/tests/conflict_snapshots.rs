//! Conflict intent is independent of whether jj materializes markers.
//! These histories model presence, bytes and explicit side selection;
//! they never learn the expected conflict set from the implementation.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::project_with;
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
use tau_testing::block_on_io as block_on;
use tau_vcs::{Identity, VcsPlugin};

#[derive(Debug, Clone, hegel::DefaultGenerator)]
enum Kind {
    Added,
    Deleted,
    ModifiedDeleted,
    Marked,
    BinaryAdded,
}

#[derive(Debug, Clone)]
struct Conflict {
    path: String,
    base: Option<Vec<u8>>,
    parent: Option<Vec<u8>>,
    child: Option<Vec<u8>>,
    /// None means genuine textual conflict, not file absence.
    materialized: Option<Vec<u8>>,
}
hegel::pretty_print_as_debug!(Conflict);

fn case(kind: Kind, path: String, payload: &str) -> Conflict {
    let base = format!("base:{payload}\n").into_bytes();
    let parent = format!("parent:{payload}\n").into_bytes();
    let child = format!("child:{payload}\n").into_bytes();
    let (base, parent, child, materialized) = match kind {
        Kind::Added => {
            (None, Some(Vec::new()), Some(child.clone()), Some(child))
        }
        Kind::Deleted => (Some(base), Some(Vec::new()), None, Some(Vec::new())),
        Kind::ModifiedDeleted => {
            (Some(Vec::new()), None, Some(child.clone()), Some(child))
        }
        Kind::Marked => (Some(base), Some(parent), Some(child), None),
        Kind::BinaryAdded => {
            let bytes = [vec![0, 255, b'c'], child].concat();
            (None, Some(Vec::new()), Some(bytes.clone()), Some(bytes))
        }
    };
    Conflict {
        path,
        base,
        parent,
        child,
        materialized,
    }
}

fn cases(payload: &str, extra: Vec<Kind>) -> Vec<Conflict> {
    // Resolving one conflict must not decide its untouched sibling. Both
    // witnesses survive shrinking; optional independent paths add normal
    // markers, additions and binary-file materializations.
    let mut cases = vec![
        case(Kind::ModifiedDeleted, "edit-me.txt".into(), payload),
        case(Kind::Deleted, "dir/deleted.txt".into(), payload),
    ];
    for (index, kind) in extra.into_iter().enumerate() {
        cases.push(case(kind, format!("dir/extra-{index} é.txt"), payload));
    }
    cases
}

#[hegel::composite]
fn conflict_cases(tc: &TestCase) -> Vec<Conflict> {
    let payload: String = tc.draw(gs::text().alphabet("ab é<>\n").max_size(24));
    let extra: Vec<Kind> = tc.draw(gs::vecs(gs::default::<Kind>()).max_size(2));
    cases(&payload, extra)
}

#[derive(Debug, Clone, hegel::DefaultGenerator)]
enum Step {
    Snapshot,
    Reopen,
    Describe,
    RewriteSame,
    DeleteThenRewrite,
    CommitUnrelated,
}

fn write(dir: &std::path::Path, path: &str, value: &Option<Vec<u8>>) {
    let path = dir.join(path);
    match value {
        Some(bytes) => {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        None if path.exists() => std::fs::remove_file(path).unwrap(),
        None => {}
    }
}

/// Accept the native materialization's metadata, not necessarily the
/// child's mode: unresolved mode changes use jj's non-executable default.
#[cfg(unix)]
#[test]
fn accepting_markerless_contents_keeps_native_executable_choice() {
    use std::os::unix::fs::PermissionsExt as _;
    for (base_exec, child_exec, expected_exec) in [
        (false, false, false),
        (false, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let home = tempfile::tempdir().unwrap();
        let project = project_with(home.path(), &[("script", "")]);
        let trunk = project.trunk().unwrap();
        let parent = project.add_workspace("parent", &trunk).unwrap();
        let base = if base_exec {
            std::fs::set_permissions(
                parent.root().join("script"),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            block_on(parent.commit_all("executable base", "tau/parent"))
                .unwrap()
                .commit_id
        } else {
            trunk
        };
        let child = project.add_workspace("child", &base).unwrap();
        std::fs::remove_file(parent.root().join("script")).unwrap();
        block_on(parent.commit_all("delete script", "tau/parent")).unwrap();
        let path = child.root().join("script");
        std::fs::write(&path, "#!/bin/sh\necho hello\n").unwrap();
        let mode = if child_exec { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))
            .unwrap();
        let edit =
            block_on(child.commit_all("edit script", "tau/child")).unwrap();
        let landing =
            block_on(parent.land(&edit.commit_id, "tau/parent", true)).unwrap();
        assert_eq!(landing.conflicts, ["script"]);
        let bytes = std::fs::read(parent.root().join("script")).unwrap();
        assert_eq!(bytes, b"#!/bin/sh\necho hello\n");
        assert_eq!(
            std::fs::metadata(parent.root().join("script"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111
                != 0,
            expected_exec
        );
        let resolve = VcsPlugin::new(parent.clone())
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "vcs_resolve")
            .unwrap();
        block_on(
            resolve.call(json!({"paths": ["script"]}), ToolCtx::detached()),
        )
        .unwrap();
        let head = project.workspace_head("parent").unwrap().unwrap();
        assert!(project.conflicts(&head).unwrap().is_empty());
        assert_eq!(
            project.file_at(&head, "script").unwrap(),
            Some((bytes, expected_exec))
        );
    }
}

/// Bounded native histories; two distinct conflict witnesses survive all
/// shrinking. Each prefix must keep the independent unresolved path set.
/// Selecting a committed side through vcs_restore is explicit resolution,
/// even when its empty bytes are identical to the materialized file.
#[hegel::test(test_cases = 20, suppress_health_check = [hegel::HealthCheck::TooSlow])]
#[hegel::explicit_test_case(conflicts = cases("", vec![]), steps = Vec::<Step>::new(), accept_materialized = false)]
#[hegel::explicit_test_case(conflicts = cases("\n<<<<<<< literal\n", vec![Kind::Added, Kind::Marked, Kind::BinaryAdded]), steps = vec![Step::RewriteSame, Step::DeleteThenRewrite, Step::Reopen], accept_materialized = true)]
fn snapshots_keep_conflicts_until_changed_or_explicitly_resolved(tc: TestCase) {
    let conflicts: Vec<Conflict> = tc.draw(conflict_cases());
    let steps: Vec<Step> = tc.draw(gs::vecs(gs::default::<Step>()).max_size(5));
    let accept_materialized: bool = tc.draw(gs::booleans());
    let home = tempfile::tempdir().unwrap();
    let base: Vec<(&str, &str)> = conflicts
        .iter()
        .filter(|case| case.path != conflicts[0].path)
        .filter_map(|case| {
            case.base.as_ref().map(|bytes| {
                (case.path.as_str(), std::str::from_utf8(bytes).unwrap())
            })
        })
        .collect();
    let project = project_with(home.path(), &base);
    let trunk = project.trunk().unwrap();
    let mut parent = project.add_workspace("parent", &trunk).unwrap();
    let dir = parent.root().to_owned();
    write(&dir, &conflicts[0].path, &conflicts[0].base);
    let checkpoint =
        block_on(parent.commit_all("common base", "tau/parent")).unwrap();
    let child = project
        .add_workspace("child", &checkpoint.commit_id)
        .unwrap();
    for case in &conflicts {
        write(&dir, &case.path, &case.parent);
        if case.child.is_some() {
            write(child.root(), &case.path, &case.child);
        }
    }
    let parent_side =
        block_on(parent.commit_all("parent side", "tau/parent")).unwrap();
    block_on(child.commit_all("child edits", "tau/child")).unwrap();
    // A stacked deletion is essential: one flat commit did not expose
    // the historical working-copy representation/cache failure.
    for case in &conflicts {
        if case.child.is_none() {
            write(child.root(), &case.path, &case.child);
        }
    }
    let child_side =
        block_on(child.commit_all("child deletions", "tau/child")).unwrap();
    let landing =
        block_on(parent.land(&child_side.commit_id, "tau/parent", true))
            .unwrap();
    let mut expected: BTreeSet<String> =
        conflicts.iter().map(|case| case.path.clone()).collect();
    assert_eq!(
        landing.conflicts.into_iter().collect::<BTreeSet<_>>(),
        expected
    );
    for case in &conflicts {
        let bytes = std::fs::read(dir.join(&case.path)).unwrap();
        match &case.materialized {
            Some(plain) => assert_eq!(&bytes, plain, "{}", case.path),
            None => {
                assert!(bytes.windows(7).any(|window| window == b"<<<<<<<"))
            }
        }
    }

    // Resolve only the first conflict by deleting its materialized file.
    // The untouched empty-v-deleted sibling must remain conflicted.
    std::fs::remove_file(dir.join(&conflicts[0].path)).unwrap();
    let edited =
        block_on(parent.commit_all("resolve one conflict", "tau/parent"))
            .unwrap();
    expected.remove(&conflicts[0].path);
    assert_eq!(
        project
            .conflicts(&edited.commit_id)
            .unwrap()
            .into_iter()
            .collect::<BTreeSet<_>>(),
        expected,
        "editing one conflict must preserve siblings"
    );
    assert_eq!(
        edited.paths,
        std::slice::from_ref(&conflicts[0].path),
        "only the edited conflict changed"
    );

    // Commit an unrelated file before the optional history as a second
    // mandatory witness, so neither preservation law can shrink away.
    for (index, step) in
        [Step::CommitUnrelated].into_iter().chain(steps).enumerate()
    {
        match step {
            Step::Snapshot => {
                block_on(parent.working_copy()).unwrap();
            }
            Step::Reopen => {
                parent = tau_testing::block_on_io(tau_vcs::Vcs::open(
                    &dir,
                    Identity::default(),
                ))
                .unwrap();
            }
            Step::Describe => {
                let tool = VcsPlugin::new(parent.clone())
                    .tools()
                    .into_iter()
                    .find(|tool| tool.name() == "vcs_describe")
                    .unwrap();
                block_on(tool.call(
                    json!({"message": "still unresolved"}),
                    ToolCtx::detached(),
                ))
                .unwrap();
            }
            Step::RewriteSame | Step::DeleteThenRewrite => {
                for case in conflicts
                    .iter()
                    .filter(|case| expected.contains(&case.path))
                {
                    let bytes = std::fs::read(dir.join(&case.path)).unwrap();
                    if matches!(step, Step::DeleteThenRewrite) {
                        std::fs::remove_file(dir.join(&case.path)).unwrap();
                    }
                    std::fs::write(dir.join(&case.path), bytes).unwrap();
                }
                block_on(parent.working_copy()).unwrap();
            }
            Step::CommitUnrelated => {
                let path = format!("unrelated-{index}.txt");
                std::fs::write(dir.join(&path), b"unrelated\n").unwrap();
                let committed =
                    block_on(parent.commit_all("unrelated work", "tau/parent"))
                        .unwrap();
                assert_eq!(
                    committed.paths,
                    [path],
                    "only the unrelated file changed"
                );
            }
        }
        let copy = block_on(parent.working_copy()).unwrap();
        assert!(
            copy.paths.is_empty(),
            "prefix {index}: no uncommitted semantic changes"
        );
        let head = project.workspace_head("parent").unwrap().unwrap();
        assert_eq!(
            project
                .conflicts(&head)
                .unwrap()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            expected,
            "prefix {index}"
        );
        let tool = VcsPlugin::new(parent.clone())
            .tools()
            .into_iter()
            .find(|tool| tool.name() == "vcs_status")
            .unwrap();
        let status =
            block_on(tool.call(json!({}), ToolCtx::detached())).unwrap();
        let details = status.details.unwrap();
        assert_eq!(details["conflicts"], json!(expected));
        assert_eq!(
            details["changes"],
            json!([]),
            "prefix {index}: status must not list redundant terms"
        );
    }

    let restore = VcsPlugin::new(parent.clone())
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_restore")
        .unwrap();
    let resolve = VcsPlugin::new(parent.clone())
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_resolve")
        .unwrap();
    if let Some(marked) = conflicts
        .iter()
        .skip(1)
        .find(|case| case.materialized.is_none())
    {
        let before = project.workspace_head("parent").unwrap().unwrap();
        let error = block_on(resolve.call(
            json!({"paths": [conflicts[1].path, marked.path]}),
            ToolCtx::detached(),
        ))
        .unwrap_err();
        assert!(error.to_string().contains("has markers"));
        assert_eq!(
            project.workspace_head("parent").unwrap().unwrap(),
            before,
            "mixed resolution must be atomic"
        );
    }
    let undo = VcsPlugin::new(parent.clone())
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_undo")
        .unwrap();
    let mut remaining = expected;
    let mut selected: BTreeMap<String, Option<Vec<u8>>> = conflicts
        .iter()
        .map(|case| (case.path.clone(), case.parent.clone()))
        .collect();
    // Select committed sides or explicitly accept native clean contents,
    // including the unchanged empty-file decision. Neither may decide siblings.
    for case in conflicts.iter().skip(1) {
        let value = if accept_materialized
            && let Some(bytes) = &case.materialized
        {
            block_on(
                resolve
                    .call(json!({"paths": [case.path]}), ToolCtx::detached()),
            )
            .unwrap();
            // Acknowledgement is a normal native operation: undo restores
            // the structured conflict, then a fresh explicit call accepts it.
            let undone =
                block_on(undo.call(json!({}), ToolCtx::detached())).unwrap();
            assert_eq!(undone.details.unwrap()["tool"], "resolve");
            let id = project.workspace_head("parent").unwrap().unwrap();
            assert_eq!(
                project
                    .conflicts(&id)
                    .unwrap()
                    .into_iter()
                    .collect::<BTreeSet<_>>(),
                remaining
            );
            assert_eq!(std::fs::read(dir.join(&case.path)).unwrap(), *bytes);
            block_on(
                resolve
                    .call(json!({"paths": [case.path]}), ToolCtx::detached()),
            )
            .unwrap();
            Some(bytes.clone())
        } else {
            block_on(restore.call(
                json!({"from": parent_side.commit_id, "paths": [case.path]}),
                ToolCtx::detached(),
            ))
            .unwrap();
            case.parent.clone()
        };
        selected.insert(case.path.clone(), value.clone());
        remaining.remove(&case.path);
        let committed =
            block_on(parent.commit_all("select parent side", "tau/parent"))
                .unwrap();
        assert_eq!(committed.paths, std::slice::from_ref(&case.path));
        let head = project.workspace_head("parent").unwrap().unwrap();
        assert_eq!(
            project
                .conflicts(&head)
                .unwrap()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            remaining
        );
        match &value {
            Some(bytes) => {
                assert_eq!(std::fs::read(dir.join(&case.path)).unwrap(), *bytes)
            }
            None => assert!(!dir.join(&case.path).exists()),
        }
    }
    let head = project.bookmark("tau/parent").unwrap().unwrap();
    for case in &conflicts {
        assert_eq!(
            project.file_at(&head, &case.path).unwrap(),
            selected[&case.path]
                .as_ref()
                .map(|bytes| (bytes.clone(), false))
        );
    }
}
