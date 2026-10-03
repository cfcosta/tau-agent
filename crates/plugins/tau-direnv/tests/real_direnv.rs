//! tau-direnv with the real direnv, when it is on `PATH` (skipped
//! otherwise): tau's whitelist loads an `.envrc` nobody ran `direnv
//! allow` on, a command sees what it exports, and a `direnv deny` still
//! holds. direnv's data directory is the test's own, so the person's
//! allow and deny records are never read or written.

use std::{path::Path, sync::Arc};

use tau_agent::launch::Launch;
use tau_direnv::{
    host::Host,
    launch::{Direnv, Status},
};
use tau_store::Store;
use tau_ui_plugin::{HostCx, RepoCtx, SavedSettings, Services};

fn run(launch: &Launch, dir: &Path, script: &str) -> String {
    let (program, args) = launch.argv("/bin/sh".as_ref(), ["-c", script]);
    let out = std::process::Command::new(program)
        .args(args)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim_end().to_owned()
}

#[test]
fn the_real_direnv_loads_through_taus_whitelist() {
    let dir = tempfile::tempdir().unwrap();
    let Some(mut direnv) = Direnv::find(&dir.path().join("tau")) else {
        eprintln!("direnv is not on PATH: skipped");
        return;
    };
    // The test's own direnv data, and none of the person's configuration.
    let data = dir.path().join("data");
    direnv
        .env
        .push(("XDG_DATA_HOME".into(), data.clone().into_os_string()));
    direnv.user_config = None;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let root = dir.path().join("repo");
    let workspace = root.join("runs/probe");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join(".envrc"), "export TAU_PROBE=1\n").unwrap();
    let repo = RepoCtx {
        name: "repo".into(),
        checkout: root.join("checkout"),
        dir: root.clone(),
        workspaces: root.clone(),
    };
    let cx = HostCx::new(
        runtime.block_on(Store::memory()).unwrap(),
        runtime.handle().clone(),
        Services::default().with(SavedSettings::in_memory()),
        dir.path().join("tau"),
        vec![repo.clone()],
        Arc::new(|_| {}),
    );
    let host = Host::with_direnv(&cx, Some(direnv.clone()));
    host.decide("repo", true).unwrap();
    let launcher = host.launcher(&repo).unwrap();
    let launch = runtime.block_on(launcher.launch(&workspace));
    assert_eq!(host.status(&workspace), Status::Ready);
    assert_eq!(run(&launch, &workspace, "echo $TAU_PROBE"), "1");

    // Denied with direnv itself, in the test's data directory: a new
    // workspace with the same `.envrc` path is not loaded.
    let denied = root.join("runs/denied");
    std::fs::create_dir_all(&denied).unwrap();
    std::fs::write(denied.join(".envrc"), "export TAU_PROBE=1\n").unwrap();
    let status = std::process::Command::new(&direnv.program)
        .arg("deny")
        .arg(&denied)
        .envs(direnv.env(&denied))
        .status()
        .unwrap();
    assert!(status.success());
    assert!(
        data.join("direnv/deny").is_dir(),
        "the deny went to the test's data"
    );
    let launch = runtime.block_on(launcher.launch(&denied));
    assert_eq!(host.status(&denied), Status::Denied);
    assert_eq!(launch, Launch::default());
}
