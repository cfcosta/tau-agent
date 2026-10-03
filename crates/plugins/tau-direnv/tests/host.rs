//! tau-direnv's host half against a fake `direnv`
//! (`tests/fixtures/fake-direnv.sh`): the person is asked once per
//! repository, commands wait while they are asked and while the
//! environment loads, then run through `direnv exec` only when allowed
//! and loaded; a failed load, a `direnv deny` and a "no" run commands
//! as they are.

use std::{
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::json;
use tau_agent::launch::{Launch, Launcher};
use tau_direnv::{
    NAME,
    host::Host,
    launch::{Direnv, Status},
};
use tau_store::Store;
use tau_ui_plugin::{HostCx, RepoCtx, SavedSettings, Services};

const FAKE: &str = include_str!("fixtures/fake-direnv.sh");

/// A repository with workspaces, a fake direnv and a host over them.
struct Fixture {
    runtime: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
    repo: RepoCtx,
    saved: SavedSettings,
}

impl Fixture {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        std::fs::create_dir_all(root.join("checkout")).unwrap();
        let repo = RepoCtx {
            name: "repo".into(),
            checkout: root.join("checkout"),
            workspaces: root.clone(),
            dir: root,
        };
        Self {
            runtime,
            dir,
            repo,
            saved: SavedSettings::in_memory(),
        }
    }

    /// The fake direnv, installed in the fixture, writing its data
    /// under the fixture too; `extra` set for every call.
    fn direnv(&self, extra: &[(&str, &str)]) -> Direnv {
        let program = self.dir.path().join("bin/direnv");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, FAKE).unwrap();
        std::fs::set_permissions(
            &program,
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut direnv = Direnv::at(program, &self.dir.path().join("tau"));
        direnv.user_config = None;
        direnv
            .env
            .push(("XDG_DATA_HOME".into(), self.data().into_os_string()));
        for (key, value) in extra {
            direnv.env.push(((*key).into(), (*value).into()));
        }
        direnv
    }

    fn data(&self) -> PathBuf {
        self.dir.path().join("data")
    }

    fn host(&self, direnv: Option<Direnv>) -> Host {
        let store = self.runtime.block_on(Store::memory()).unwrap();
        let cx = HostCx::new(
            store,
            self.runtime.handle().clone(),
            Services::default().with(self.saved.clone()),
            self.dir.path().join("tau"),
            vec![self.repo.clone()],
            Arc::new(|_| {}),
        );
        Host::with_direnv(&cx, direnv)
    }

    /// A workspace of the repository, with `envrc` as its `.envrc`.
    fn workspace(&self, name: &str, envrc: Option<&str>) -> PathBuf {
        let dir = self.repo.workspaces.join("runs").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if let Some(envrc) = envrc {
            std::fs::write(dir.join(".envrc"), envrc).unwrap();
        }
        dir
    }

    fn launch(&self, launcher: &Arc<dyn Launcher>, dir: &Path) -> Launch {
        self.runtime.block_on(launcher.launch(dir))
    }
}

/// What `script` prints, run in `dir` the way `launch` says.
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

const PROBE: &str = "export TAU_PROBE=1\n";

fn wait_for(host: &Host, dir: &Path, status: &Status) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while &host.status(dir) != status {
        assert!(
            Instant::now() < deadline,
            "{:?}, not {status:?}",
            host.status(dir)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The first command waits for the person; once they allow the
/// repository, it runs in the environment, the answer is kept, and the
/// repository's other workspaces are not asked again.
#[test]
fn the_person_is_asked_once_then_commands_run_in_the_environment() {
    let fixture = Fixture::new();
    let host = fixture.host(Some(fixture.direnv(&[])));
    let launcher = host.launcher(&fixture.repo).expect("direnv is installed");
    let first = fixture.workspace("first", Some(PROBE));

    let waiting = {
        let launcher = launcher.clone();
        let first = first.clone();
        fixture
            .runtime
            .spawn(async move { launcher.launch(&first).await })
    };
    wait_for(&host, &first, &Status::Asking);
    std::thread::sleep(Duration::from_millis(100));
    assert!(!waiting.is_finished(), "the command waits for the answer");

    host.decide("repo", true).unwrap();
    let launch = fixture.runtime.block_on(waiting).unwrap();
    assert_eq!(
        launch.prefix[1..3],
        ["exec".into(), first.clone().into_os_string()]
    );
    assert_eq!(run(&launch, &first, "echo $TAU_PROBE"), "1");
    assert_eq!(host.status(&first), Status::Ready);
    assert_eq!(
        fixture.saved.read(NAME),
        Some(json!({ "repos": { "repo": true } }))
    );

    // Another workspace loads without asking.
    let second = fixture.workspace("second", Some(PROBE));
    let launch = fixture.launch(&launcher, &second);
    assert_eq!(run(&launch, &second, "echo $TAU_PROBE"), "1");

    // A host made again keeps the answer.
    let again = fixture.host(Some(fixture.direnv(&[])));
    let third = fixture.workspace("third", Some(PROBE));
    let launch =
        fixture.launch(&again.launcher(&fixture.repo).unwrap(), &third);
    assert_eq!(run(&launch, &third, "echo $TAU_PROBE"), "1");
}

/// "Run without it": commands run as they are, and the environment is
/// never loaded.
#[test]
fn without_the_persons_leave_commands_run_as_they_are() {
    let fixture = Fixture::new();
    let log = fixture.dir.path().join("calls");
    let host = fixture.host(Some(
        fixture.direnv(&[("FAKE_DIRENV_LOG", log.to_str().unwrap())]),
    ));
    let launcher = host.launcher(&fixture.repo).unwrap();
    let dir = fixture.workspace("a", Some(PROBE));
    host.decide("repo", false).unwrap();
    let launch = fixture.launch(&launcher, &dir);
    assert_eq!(launch, Launch::default());
    assert_eq!(run(&launch, &dir, "echo ${TAU_PROBE:-none}"), "none");
    assert_eq!(host.status(&dir), Status::Off);
    assert!(!log.exists(), "direnv never ran");

    // Allowed later from the menu: it loads.
    host.decide("repo", true).unwrap();
    wait_for(&host, &dir, &Status::Ready);
    let launch = fixture.launch(&launcher, &dir);
    assert_eq!(run(&launch, &dir, "echo $TAU_PROBE"), "1");
}

/// A command waits while the environment loads.
#[test]
fn a_command_waits_while_the_environment_loads() {
    let fixture = Fixture::new();
    let host =
        fixture.host(Some(fixture.direnv(&[("FAKE_DIRENV_DELAY", "1")])));
    host.decide("repo", true).unwrap();
    let launcher = host.launcher(&fixture.repo).unwrap();
    let dir = fixture.workspace("slow", Some(PROBE));
    let started = Instant::now();
    let waiting = {
        let (launcher, dir) = (launcher.clone(), dir.clone());
        fixture
            .runtime
            .spawn(async move { launcher.launch(&dir).await })
    };
    wait_for(&host, &dir, &Status::Loading);
    let launch = fixture.runtime.block_on(waiting).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert_eq!(run(&launch, &dir, "echo $TAU_PROBE"), "1");
}

/// A load that fails says how, with direnv's last lines, and commands
/// run without the environment.
#[test]
fn a_failed_load_runs_commands_without_it() {
    let fixture = Fixture::new();
    let host = fixture.host(Some(fixture.direnv(&[])));
    host.decide("repo", true).unwrap();
    let launcher = host.launcher(&fixture.repo).unwrap();
    let dir =
        fixture.workspace("broken", Some("echo 'no flake here' >&2\nexit 3\n"));
    let launch = fixture.launch(&launcher, &dir);
    assert_eq!(launch, Launch::default());
    assert_eq!(
        host.status(&dir),
        Status::Failed {
            status: "direnv exited 3".into(),
            output: "no flake here".into(),
        }
    );
}

/// An `.envrc` the person denied with direnv is not loaded, allowed or
/// not.
#[test]
fn a_denied_envrc_is_not_loaded() {
    let fixture = Fixture::new();
    let host = fixture.host(Some(fixture.direnv(&[])));
    host.decide("repo", true).unwrap();
    let dir = fixture.workspace("denied", Some(PROBE));
    let record = dir.join(".envrc");
    let hash = {
        use sha2::{Digest as _, Sha256};
        let digest =
            Sha256::digest(format!("{}\n", record.display()).as_bytes());
        digest
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let deny = fixture.data().join("direnv/deny");
    std::fs::create_dir_all(&deny).unwrap();
    std::fs::write(deny.join(hash), record.display().to_string()).unwrap();

    let launch = fixture.launch(&host.launcher(&fixture.repo).unwrap(), &dir);
    assert_eq!(launch, Launch::default());
    assert_eq!(host.status(&dir), Status::Denied);
}

/// Without direnv the plugin is off; without an `.envrc` nothing waits.
#[test]
fn nothing_to_load_changes_nothing() {
    let fixture = Fixture::new();
    let host = fixture.host(None);
    assert!(host.launcher(&fixture.repo).is_none());
    assert!(!host.repo_data(&fixture.repo).direnv);

    let host = fixture.host(Some(fixture.direnv(&[])));
    let dir = fixture.workspace("plain", None);
    let launch = fixture.launch(&host.launcher(&fixture.repo).unwrap(), &dir);
    assert_eq!(launch, Launch::default());
    assert!(host.repo_data(&fixture.repo).direnv);
    assert!(!host.repo_data(&fixture.repo).envrc);
}

/// tau's configuration is the person's with the allowed repositories
/// whitelisted, and loses them once they are no longer allowed.
#[test]
fn the_configuration_mirrors_the_persons_and_allows_the_repositories() {
    let fixture = Fixture::new();
    let user = fixture.dir.path().join("user");
    std::fs::create_dir_all(user.join("lib")).unwrap();
    std::fs::write(user.join("direnv.toml"), "[global]\nload_dotenv = true\n")
        .unwrap();
    let mut direnv = fixture.direnv(&[]);
    direnv.user_config = Some(user.clone());
    let config = direnv.config.join("direnv.toml");
    let host = fixture.host(Some(direnv.clone()));
    host.launcher(&fixture.repo).unwrap();
    let read = || -> toml::Table {
        std::fs::read_to_string(&config).unwrap().parse().unwrap()
    };
    assert_eq!(read()["global"]["load_dotenv"].as_bool(), Some(true));
    assert_eq!(read()["whitelist"]["prefix"].as_array().unwrap().len(), 0);
    assert_eq!(
        std::fs::read_link(direnv.config.join("lib")).unwrap(),
        user.join("lib")
    );

    host.decide("repo", true).unwrap();
    let prefixes = read()["whitelist"]["prefix"].clone();
    assert_eq!(
        prefixes,
        toml::Value::Array(vec![
            fixture.repo.workspaces.display().to_string().into()
        ])
    );
    host.decide("repo", false).unwrap();
    assert_eq!(read()["whitelist"]["prefix"].as_array().unwrap().len(), 0);
}
