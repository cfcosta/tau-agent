//! tau-direnv in the host, with a fake `direnv` first on `PATH`: a
//! chat's `bash` waits while the person is asked, then sees the
//! repository's environment only once they allowed it; a later chat is
//! not asked again; "Run without it" leaves commands as they were; a
//! failed load says so and Try again loads it; and the repository's MCP
//! servers start through it too.
//!
//! `PATH` and the XDG directories are set once for this whole binary,
//! before any host looks for direnv, to the same values for every test.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::{Duration, Instant},
};

use serde_json::json;
use tau_agent::{agent::Agent, event::RunEvent, tool::RunId};
use tau_direnv::{Act, NAME, Record};
use tau_testing::{git::git, scripted::ScriptedModel};
use tau_ui::{
    accounts::Credentials,
    host::{Host, HostConfig},
};
use tau_ui_remote::{models::ModelChoice, view::Item};
use tau_vcs_host::{Identity, Project};
use tokio::sync::mpsc::UnboundedReceiver;

const REPO: &str = "repo";
const WAIT: Duration = Duration::from_secs(60);

/// The fake direnv's directory, first on `PATH`, and the XDG
/// directories, all the test binary's own.
fn environment() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap().keep();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("direnv");
        std::fs::write(
            &fake,
            include_str!(
                "../../plugins/tau-direnv/tests/fixtures/fake-direnv.sh"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let path = std::env::join_paths(std::iter::once(bin).chain(
            std::env::split_paths(
                &std::env::var_os("PATH").unwrap_or_default(),
            ),
        ))
        .unwrap();
        // SAFETY: set once, before any host or thread of this binary's
        // tests reads the environment, to the same values for all.
        unsafe {
            std::env::set_var("PATH", path);
            std::env::set_var("XDG_DATA_HOME", dir.join("data"));
            std::env::set_var("XDG_CONFIG_HOME", dir.join("config"));
            std::env::remove_var("DIRENV_CONFIG");
        }
        dir
    })
}

/// A host on `llm` over a repository whose checkout has `envrc` as its
/// `.envrc`, with the data directory it keeps.
fn host(
    llm: ScriptedModel,
    envrc: &str,
) -> (Host, UnboundedReceiver<RunEvent>, PathBuf) {
    environment();
    let checkout = tempfile::tempdir().unwrap().keep();
    git(&checkout, &["init", "--quiet"]);
    std::fs::write(checkout.join(".envrc"), envrc).unwrap();
    git(&checkout, &["add", ".envrc"]);
    git(&checkout, &["commit", "--quiet", "-m", "first"]);
    let data = tempfile::tempdir().unwrap().keep();
    let project = tau_vcs_host::ProjectRepo::import(
        checkout.to_str().unwrap(),
        data.join("repos/repo"),
        Identity::default(),
    )
    .map(Project::from)
    .unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(tau_store_sqlite::memory()).unwrap();
    let config = HostConfig {
        account: tau_ai::chatgpt::AccountId::parse("test-account").unwrap(),
        credentials: Credentials::new(data.join("config")),
        model: Some("gpt-6-luna".into()),
        store: data.join("runs.db"),
        repos: data.join("repos"),
        settings: data.join("models.json"),
        repo_list: data.join("repos.json"),
        skills: std::env::temp_dir().join("tau-test-skills-none"),
    };
    let (host, events) =
        Host::with_agent(runtime, Agent::new(llm).name("coder"), store, config);
    (host.with_repo(REPO, project), events, data)
}

/// A turn running `command` with bash, then one answering.
fn bash_then_done(llm: ScriptedModel, command: &str) -> ScriptedModel {
    let command = command.to_owned();
    llm.turn(move |t| t.tool_call("bash", json!({ "command": command })))
        .turn(|t| t.text("done"))
}

const PROBE: &str = "echo probe=${TAU_PROBE:-none}";

/// tau-direnv's records for `run`, as stored.
fn records(host: &Host, run: &RunId) -> Vec<Record> {
    host.block_on(host.plugin_records(run, NAME))
        .into_iter()
        .filter_map(|body| serde_json::from_value(body).ok())
        .collect()
}

fn wait_for_record(
    host: &Host,
    run: &RunId,
    found: impl Fn(&Record) -> bool,
) -> Record {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(record) = records(host, run)
            .into_iter()
            .rev()
            .find(|record| found(record))
        {
            return record;
        }
        assert!(
            Instant::now() < deadline,
            "no such record: {:?}",
            records(host, run)
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn act(host: &Host, act: Act) {
    host.block_on(host.plugin_act(NAME, serde_json::to_value(act).unwrap()))
        .unwrap();
}

/// The text bash gave the model in `run`, once it ended.
fn bash_output(
    host: &Host,
    run: &RunId,
    events: &mut UnboundedReceiver<RunEvent>,
) -> String {
    let mut view = tau_ui_remote::view::RunView::new(
        run.clone(),
        "t",
        "coder",
        "gpt-6-luna",
    );
    let deadline = Instant::now() + WAIT;
    loop {
        match events.try_recv() {
            Ok(event) => {
                let end = matches!(&event, RunEvent::RunEnd { run: ended, .. } if ended == run);
                view.apply(&event);
                if end {
                    break;
                }
            }
            Err(_) => {
                assert!(Instant::now() < deadline, "the run did not end");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    while host.is_running(run) {
        std::thread::sleep(Duration::from_millis(5));
    }
    view.items
        .iter()
        .find_map(|item| match item {
            Item::Tool(card) if card.tool == "bash" => {
                card.data.result.as_ref().map(|result| result.text.clone())
            }
            _ => None,
        })
        .expect("bash ran")
}

/// The first chat's command waits while the person is asked, with the
/// `.envrc`; once they allow it, it runs in the environment. The next
/// chat is not asked, and its command runs in it from the start.
#[test]
fn bash_sees_the_environment_only_once_the_person_allows_it() {
    let llm =
        bash_then_done(bash_then_done(ScriptedModel::new(), PROBE), PROBE);
    let (host, mut events, _data) = host(llm, "export TAU_PROBE=1\n");
    let first = host
        .block_on(host.start("check the probe", &ModelChoice::default(), REPO))
        .unwrap();
    let asked = wait_for_record(&host, &first.id, |r| {
        matches!(r, Record::Asked { .. })
    });
    assert_eq!(
        asked,
        Record::Asked {
            repo: REPO.into(),
            envrc: "export TAU_PROBE=1\n".into()
        }
    );
    assert!(host.is_running(&first.id), "bash waits for the answer");

    act(
        &host,
        Act::Decide {
            repo: REPO.into(),
            load: true,
        },
    );
    let output = bash_output(&host, &first.id, &mut events);
    assert!(output.contains("probe=1"), "{output}");
    let kinds: Vec<Record> = records(&host, &first.id);
    assert!(kinds.contains(&Record::Loaded), "{kinds:?}");

    let second = host
        .block_on(host.start("check it again", &ModelChoice::default(), REPO))
        .unwrap();
    let output = bash_output(&host, &second.id, &mut events);
    assert!(output.contains("probe=1"), "{output}");
    assert!(
        !records(&host, &second.id)
            .iter()
            .any(|r| matches!(r, Record::Asked { .. })),
        "asked once per repository"
    );
}

/// "Run without it": the waiting command runs as it would have.
#[test]
fn run_without_it_leaves_commands_as_they_were() {
    let llm = bash_then_done(ScriptedModel::new(), PROBE);
    let (host, mut events, _data) = host(llm, "export TAU_PROBE=1\n");
    let run = host
        .block_on(host.start("check the probe", &ModelChoice::default(), REPO))
        .unwrap();
    wait_for_record(&host, &run.id, |r| matches!(r, Record::Asked { .. }));
    act(
        &host,
        Act::Decide {
            repo: REPO.into(),
            load: false,
        },
    );
    let output = bash_output(&host, &run.id, &mut events);
    assert!(output.contains("probe=none"), "{output}");
    assert!(records(&host, &run.id).contains(&Record::Off));
}

/// A load that fails says why, and the command runs without it; Try
/// again loads the workspace once the `.envrc` is fixed.
#[test]
fn a_failed_load_runs_without_it_until_tried_again() {
    let llm = bash_then_done(ScriptedModel::new(), PROBE);
    let (host, mut events, _data) =
        host(llm, "echo 'no devShell here' >&2\nexit 1\n");
    let run = host
        .block_on(host.start("check the probe", &ModelChoice::default(), REPO))
        .unwrap();
    wait_for_record(&host, &run.id, |r| matches!(r, Record::Asked { .. }));
    act(
        &host,
        Act::Decide {
            repo: REPO.into(),
            load: true,
        },
    );
    let failed =
        wait_for_record(&host, &run.id, |r| matches!(r, Record::Failed { .. }));
    assert_eq!(
        failed,
        Record::Failed {
            status: "direnv exited 1".into(),
            output: "no devShell here".into()
        }
    );
    let output = bash_output(&host, &run.id, &mut events);
    assert!(output.contains("probe=none"), "{output}");

    let workspace = host.block_on(host.workspace(&run.id)).unwrap();
    std::fs::write(workspace.join(".envrc"), "export TAU_PROBE=1\n").unwrap();
    act(
        &host,
        Act::Reload {
            run: run.id.0.to_string(),
        },
    );
    wait_for_record(&host, &run.id, |r| *r == Record::Loaded);
}

/// A repository's MCP server, of the user's but run per repository,
/// starts through direnv in the main workspace once allowed.
#[test]
fn the_repositorys_mcp_servers_start_in_the_environment() {
    let llm = bash_then_done(ScriptedModel::new(), PROBE);
    let (host, mut events, data) = host(llm, "export TAU_PROBE=1\n");
    let seen = data.join("mcp-saw");
    std::fs::create_dir_all(data.join("config")).unwrap();
    std::fs::write(
        data.join("config/mcp.json"),
        json!({ "mcpServers": { "probe": {
            "command": "/bin/sh",
            "args": ["-c", format!("echo ${{TAU_PROBE:-none}} > {}", seen.display())],
            "cwd": "."
        }}})
        .to_string(),
    )
    .unwrap();
    // The main chat goes on: its turn brings main's files, the
    // `.envrc` among them.
    let main = host.block_on(host.main_of(REPO)).unwrap();
    host.block_on(host.resume(
        &main,
        "check the probe",
        &ModelChoice::default(),
    ))
    .unwrap();
    wait_for_record(&host, &main, |r| matches!(r, Record::Asked { .. }));
    act(
        &host,
        Act::Decide {
            repo: REPO.into(),
            load: true,
        },
    );
    bash_output(&host, &main, &mut events);
    let deadline = Instant::now() + WAIT;
    while !seen.exists() {
        assert!(Instant::now() < deadline, "the server never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(std::fs::read_to_string(&seen).unwrap().trim(), "1");
}
