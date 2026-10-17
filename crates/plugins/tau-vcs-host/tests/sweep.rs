//! A sweep on a real project (`docs/reference/vcs.md`, "Sweeping at
//! start"): what a kept run has stays, what a landed run left goes with
//! its commits kept, a discarded run's commits go with its workspace and
//! head, and a workspace or head no run owns goes. Sweeping again finds
//! nothing.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use jj_lib::{
    backend::ChangeId,
    default_backend_factories::{
        default_backend_factories,
        default_working_copy_factories,
    },
    repo::Repo as _,
    workspace::Workspace,
};
use tau_agent::tool::RunId;
use tau_testing::block_on_io as block_on;
use tau_vcs_host::{
    Head,
    MAIN_WORKSPACE,
    ProjectRepo,
    Vcs,
    sweep::{Keep, Owner, Standing},
};

/// A run's workspace `name` on trunk, with `file` committed under the
/// run's head. Returns the commit's change id.
fn run_with_commit(
    project: &ProjectRepo,
    run: &str,
    name: &str,
    file: &str,
) -> (Vcs, String) {
    let vcs = project
        .add_workspace(name, &project.trunk().unwrap())
        .unwrap();
    std::fs::write(project.workspace_dir(name).join(file), "x\n").unwrap();
    let committed =
        block_on(vcs.commit_all(format!("feat: {file}"), head(run))).unwrap();
    (vcs, committed.change_id)
}

fn head(run: &str) -> Head {
    Head::Run(RunId(run.into()))
}

/// Whether a visible commit holds the change `change` (reverse hex).
fn visible(project: &ProjectRepo, change: &str) -> bool {
    let settings = jj_lib::settings::UserSettings::from_config_and_home_dir(
        jj_lib::config::StackedConfig::with_defaults(),
        None,
    )
    .unwrap();
    let workspace = Workspace::load(
        &settings,
        project.main_dir(),
        &default_backend_factories(),
        &default_working_copy_factories(),
    )
    .unwrap();
    let repo = block_on(workspace.repo_loader().load_at_head()).unwrap();
    let change = ChangeId::try_from_reverse_hex(change).unwrap();
    block_on(repo.resolve_change_id(&change))
        .unwrap()
        .is_some_and(|targets| targets.visible_with_offsets().next().is_some())
}

#[test]
fn a_sweep_takes_what_no_kept_run_has() {
    let home = tempfile::tempdir().unwrap();
    let project = common::project(home.path());
    let (_kept, kept_change) = run_with_commit(&project, "k", "wk", "k.txt");
    let (_landed, landed_change) =
        run_with_commit(&project, "l", "wl", "l.txt");
    let (_dropped, dropped_change) =
        run_with_commit(&project, "d", "wd", "d.txt");
    // The dropped run left work in `@` too, and a workspace and head
    // belong to a run the store no longer has.
    std::fs::write(project.workspace_dir("wd").join("more.txt"), "y\n")
        .unwrap();
    let (_stray, stray_change) =
        run_with_commit(&project, "gone", "stray", "s.txt");

    let main = Keep::Workspace(MAIN_WORKSPACE.to_owned());
    let owner = |run: &str, standing, workspace: &str| Owner {
        run: run.to_owned(),
        standing,
        workspaces: vec![workspace.to_owned()],
        keep: main.clone(),
    };
    let owners = [
        owner("k", Standing::Kept, "wk"),
        owner("l", Standing::Landed, "wl"),
        owner("d", Standing::Discarded, "wd"),
    ];
    let sweep = project.plan_sweep(&owners).unwrap();
    assert_eq!(sweep.forget, ["stray", "wd", "wl"]);
    let names = |runs: &[&str]| -> Vec<String> {
        runs.iter().map(|run| head(run).to_string()).collect()
    };
    assert_eq!(sweep.remove, names(&["d", "gone", "l"]));
    assert_eq!(sweep.abandon.len(), 1);
    project.sweep(&sweep).unwrap();

    assert_eq!(project.workspaces().unwrap(), ["wk"]);
    assert_eq!(project.run_heads().unwrap(), names(&["k"]));
    assert!(project.head(&head("k")).unwrap().is_some());
    for gone in ["d", "gone", "l"] {
        assert_eq!(project.head(&head(gone)).unwrap(), None, "{gone}");
    }
    for gone in ["wd", "wl", "stray"] {
        assert!(!project.workspace_dir(gone).exists(), "{gone}");
    }
    assert!(project.main_dir().to_owned().exists());
    // Only the discarded run's commits went.
    assert!(visible(&project, &kept_change));
    assert!(visible(&project, &landed_change));
    assert!(
        visible(&project, &stray_change),
        "a gone run's are not known"
    );
    assert!(!visible(&project, &dropped_change));
    // Done once, there is nothing left to do.
    assert!(project.plan_sweep(&owners).unwrap().is_empty());
    // Every operation, the sweep's included, is tau's.
    let mut log = common::ops::OpLog::open(
        &project.main_dir().to_owned().join(".jj/repo"),
    );
    common::ops::assert_all_taus(&mut log, "the sweep");
}
