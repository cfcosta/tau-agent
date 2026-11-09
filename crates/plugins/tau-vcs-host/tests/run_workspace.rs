//! A run's workspace as the kernel asks it ([`JjRunWorkspace`], through
//! `Repository::run_workspace`): a main chat's commits move trunk, a
//! chat proposes its landing with `vcs_land`, and a main chat's and a
//! sub-agent's `vcs_land` refuse, in jj's words.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::sync::{Arc, Mutex};

use common::project_with;
use serde_json::json;
use tau_agent::agent::Agent;
use tau_ai::message::{InputBlock, Message};
use tau_testing::scripted::ScriptedModel;
use tau_tools_host::{path::Root, plugin::CodingTools};
use tau_ui_plugin::{
    RunKind,
    vcs::{Repository as _, RunSpec, RunWorkspace},
};
use tau_vcs_host::{
    Head,
    Identity,
    JjRepository,
    MAIN_DOES_NOT_LAND,
    SUB_AGENTS_DO_NOT_LAND,
};

use crate::common;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// The coding agent on `workspace`, with the VCS's parts after the
/// coding tools, as the host builds a run.
fn coder(llm: ScriptedModel, workspace: &dyn RunWorkspace) -> Agent {
    let agent = Agent::new(llm)
        .name("coder")
        .plugin(CodingTools::new(Root::new(workspace.dir())));
    workspace
        .agent_plugins()
        .into_iter()
        .fold(agent, Agent::boxed_plugin)
}

/// What a tool result says, and whether it failed.
fn result_of(message: Option<&Message>) -> (String, bool) {
    let Some(Message::ToolResult(result)) = message else {
        panic!("{message:?} is no tool result")
    };
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect();
    (text, result.is_error)
}

/// Each kind of run writes a file, commits it and calls `vcs_land`: a
/// main chat's commit moves trunk and it has no head of its own; a
/// chat's commit stays on its own head and its `vcs_land` proposes the
/// landing; a main chat's and a sub-agent's `vcs_land` refuse. Every
/// run's turn hooks hear the turn that wrote the file.
#[test]
fn a_run_workspace_commits_and_lands_as_its_kind_says() {
    for kind in [RunKind::Main, RunKind::Chat, RunKind::SubAgent] {
        let home = tempfile::tempdir().unwrap();
        let project = project_with(home.path(), &[("README.md", "hello\n")]);
        let repository =
            JjRepository::new(project.clone().into(), Identity::default());
        let before = project.trunk().unwrap();
        runtime().block_on(async {
            let store = tau_store_sqlite::memory().await.unwrap();
            let workspace = repository
                .run_workspace(RunSpec {
                    name: "run".into(),
                    kind,
                })
                .await
                .unwrap();
            let heard = Arc::new(Mutex::new(Vec::new()));
            {
                let heard = heard.clone();
                workspace.turns().on_turn(move |turn| {
                    heard.lock().unwrap().extend(turn.paths.clone());
                });
            }
            let llm = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call(
                        "write",
                        json!({ "path": "new.txt", "content": "new\n" }),
                    )
                })
                .turn(|t| {
                    t.tool_call("vcs_commit", json!({ "message": "feat: new" }))
                })
                .turn(|t| t.tool_call("vcs_land", json!({})))
                .turn(|t| t.text("done"));
            let outcome = coder(llm.clone(), workspace.as_ref())
                .run("write new.txt", &store)
                .await
                .unwrap();
            assert_eq!(outcome.text, "done", "{kind:?}");
            assert!(
                heard.lock().unwrap().contains(&"new.txt".to_owned()),
                "{kind:?}: {heard:?}"
            );

            let (land, failed) = result_of(llm.requests()[3].transcript.last());
            match kind {
                RunKind::Main => {
                    assert!(failed, "{land}");
                    assert!(land.contains(MAIN_DOES_NOT_LAND), "{land}");
                }
                RunKind::SubAgent => {
                    assert!(failed, "{land}");
                    assert!(land.contains(SUB_AGENTS_DO_NOT_LAND), "{land}");
                }
                RunKind::Chat => {
                    assert!(!failed, "{land}");
                    assert!(land.contains("Proposed landing"), "{land}");
                }
            }

            let after = project.trunk().unwrap();
            let own = project.head(&Head::Run(outcome.run.clone())).unwrap();
            if kind == RunKind::Main {
                assert_ne!(after, before);
                let changed: Vec<String> = project
                    .diff(&before, &after)
                    .unwrap()
                    .into_iter()
                    .map(|file| file.path)
                    .collect();
                assert_eq!(changed, ["new.txt"]);
                assert_eq!(own, None);
            } else {
                assert_eq!(after, before, "{kind:?}");
                let own = own.expect("the run's own head");
                let changed: Vec<String> = project
                    .diff(&before, &own)
                    .unwrap()
                    .into_iter()
                    .map(|file| file.path)
                    .collect();
                assert_eq!(changed, ["new.txt"], "{kind:?}");
            }
        });
    }
}
