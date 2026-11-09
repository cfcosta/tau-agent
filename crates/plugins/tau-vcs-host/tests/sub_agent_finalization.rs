//! Finalization is a handoff, not permission to discard a child's edits.
//! Generated operation histories carry an independent path -> bytes model.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::collections::BTreeMap;

use async_trait::async_trait;
use common::{
    caller_in,
    coder_in,
    heard_sub_agents,
    notes,
    project_with,
    ready_child,
};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{
    limits::Limits,
    plugin::{FinishedRun, Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::RunId,
};
use tau_testing::scripted::ScriptedModel;
use tau_ui_plugin::{
    sub_agents::{Ending, Spawn},
    vcs::{Link, WORKSPACE_PLUGIN},
};
use tau_vcs_host::{Head, Identity, Vcs};

use crate::common;

/// A real jj-lib refusal, not a mocked commit result: tag the already
/// snapshotted working copy immediately before the workspace finishes.
#[derive(Clone)]
struct TagOnFinish(tau_vcs_host::Vcs);

#[async_trait]
impl Plugin for TagOnFinish {
    fn name(&self) -> &str {
        "tag-on-finish"
    }
    async fn start(
        &self,
        _plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(self.clone()))
    }
}

#[async_trait]
impl PluginRun for TagOnFinish {
    async fn finish(&mut self, _run: &FinishedRun<'_>, _ctx: &PluginCtx) {
        self.0.working_copy().await.unwrap();
        let dir = self.0.root().to_owned();
        tokio::task::spawn_blocking(move || {
            use jj_lib::{
                config::{ConfigLayer, ConfigSource, StackedConfig},
                default_backend_factories::{
                    default_backend_factories,
                    default_working_copy_factories,
                },
                op_store::RefTarget,
                ref_name::RefName,
                settings::UserSettings,
            };
            let mut config = StackedConfig::with_defaults();
            let mut user = ConfigLayer::empty(ConfigSource::User);
            user.set_value("user.name", "fixture").unwrap();
            user.set_value("user.email", "fixture@example.invalid")
                .unwrap();
            config.add_layer(user);
            let settings =
                UserSettings::from_config_and_home_dir(config, None).unwrap();
            let workspace = jj_lib::workspace::WorkspaceLoaderFactory::create(
                &jj_lib::workspace::DefaultWorkspaceLoaderFactory,
                &dir,
            )
            .unwrap()
            .load(
                &settings,
                &default_backend_factories(),
                &default_working_copy_factories(),
            )
            .unwrap();
            let repo =
                pollster::block_on(workspace.repo_loader().load_at_head(None))
                    .unwrap();
            let id = repo
                .view()
                .get_wc_commit_id(workspace.workspace_name())
                .unwrap()
                .clone();
            let mut tx = repo.start_transaction();
            tx.repo_mut().set_local_tag_target(
                RefName::new("blocked-finalization"),
                RefTarget::normal(id),
            );
            pollster::block_on(
                tx.commit("fixture: protect child working copy"),
            )
            .unwrap();
        })
        .await
        .unwrap();
    }
}

/// A forced pending write survives shrinking. Prior commits and extra writes
/// shrink independently; empty, whitespace, failed and successful final replies
/// are finite partners, all exercised for every generated history.
#[hegel::test(test_cases = 12, suppress_health_check = [hegel::HealthCheck::TooSlow])]
#[hegel::explicit_test_case(committed = 0_usize, extra = Vec::<String>::new(), pending = String::new(), limited = false)]
#[hegel::explicit_test_case(committed = 1_usize, extra = vec![String::from("é\n")], pending = String::new(), limited = true)]
fn finalization_commits_exact_bytes_or_keeps_the_child_workspace(tc: TestCase) {
    let committed: usize = tc.draw(gs::integers().max_value(2));
    let extra: Vec<String> = tc
        .draw(gs::vecs(gs::text().alphabet("ab é\n").max_size(24)).max_size(3));
    let pending: String = tc.draw(gs::text().alphabet("ab é\n").max_size(32));
    let limited: bool = tc.draw(gs::booleans());
    for mode in 0..5 {
        let home = tempfile::tempdir().unwrap();
        let project = project_with(home.path(), &[("existing.txt", "base\n")]);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = tau_store_sqlite::memory().await.unwrap();
            let parent = caller_in(&project, "parent");
            let llm = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call(
                        "spawn",
                        json!({"task": "write and commit the generated files"}),
                    )
                })
                .turn(|t| t.text("caller done"));
            let mut expected = BTreeMap::from([(
                "existing.txt".to_owned(),
                "base\n".to_owned(),
            )]);
            let mut child = ScriptedModel::new();
            for index in 0..committed {
                let path = format!("committed-{index}.txt");
                let content = format!("committed {index}\n");
                expected.insert(path.clone(), content.clone());
                child = child.turn(move |t| {
                    t.tool_call(
                        "write",
                        json!({"path": path, "content": content}),
                    )
                    .tool_call(
                        "vcs_commit",
                        json!({"message": format!("feat: committed {index}")}),
                    )
                });
            }
            let mut writes = Vec::new();
            for (index, content) in extra.iter().enumerate() {
                let path = format!("extra-{index}.txt");
                expected.insert(path.clone(), content.clone());
                writes.push(json!({"path": path, "content": content}));
            }
            let content = format!("pending:{pending}");
            expected.insert("pending.txt".to_owned(), content.clone());
            writes.push(json!({"path": "pending.txt", "content": content}));
            child = child.turn(move |mut t| {
                for args in writes {
                    t = t.tool_call("write", args);
                }
                t
            });
            if !limited {
                child = child
                    .turn(|t| t.text("done"))
                    .turn(|t| t.text("still done"));
            }
            child = match mode {
                0 => child.turn(|t| t.text("")),
                1 => child.turn(|t| t.text(" \n\t")),
                2 => child.turn(|t| {
                    t.error(
                        "invalid_request_error",
                        "description request failed",
                    )
                }),
                _ => child.turn(|t| t.text("feat: pending files")),
            };
            let child_script = child.clone();
            let (agents, mut ends) = heard_sub_agents();
            let agent =
                coder_in(llm.clone(), parent.as_ref()).tool(Spawn::new(
                    parent.clone(),
                    agents,
                    &notes(),
                    &[],
                    ready_child(move |workspace, _| {
                        let agent = if mode == 4 {
                            // Tags before the workspace commits at the end.
                            let tag = TagOnFinish(Vcs::lazy(
                                workspace.dir(),
                                Identity::default(),
                            ));
                            let agent = tau_agent::agent::Agent::new(
                                child_script.clone(),
                            )
                            .plugin(tag)
                            .plugin(
                                tau_tools_host::plugin::CodingTools::new(
                                    tau_tools_host::path::Root::new(
                                        workspace.dir(),
                                    ),
                                ),
                            );
                            workspace.agent_plugins().into_iter().fold(
                                agent,
                                tau_agent::agent::Agent::boxed_plugin,
                            )
                        } else {
                            coder_in(child_script.clone(), workspace.as_ref())
                        };
                        Ok(if limited {
                            agent.limits(
                                Limits::default()
                                    .max_turns(committed as u32 + 1),
                            )
                        } else {
                            agent
                        })
                    }),
                ));
            let outcome =
                agent.run("hand the writes over", &store).await.unwrap();
            let (_, ending) = ends.recv().await.unwrap();
            let children = store.subagents(&outcome.run.0).await.unwrap();
            assert_eq!(children.len(), 1);
            let child_run = &children[0];
            let records = store
                .plugin_entries(&child_run.id, WORKSPACE_PLUGIN)
                .await
                .unwrap();
            let link = records
                .iter()
                .filter_map(|(_, body)| Link::parse(body))
                .next_back()
                .unwrap();
            let child_dir = project.workspace_dir(&link.workspace);
            let successful = mode == 3;
            // Nothing lands here: the host lands what ended. What ends
            // holds every byte, committed, or keeps its workspace.
            match &ending {
                Ending::Done { .. } => assert!(successful, "mode={mode}"),
                Ending::Retained { workspace, .. } => {
                    assert!(!successful, "mode={mode}");
                    assert_eq!(*workspace, child_dir);
                }
                ending => panic!("{ending:?}, mode={mode}"),
            }
            assert!(child_dir.exists());
            for (path, content) in &expected {
                assert_eq!(
                    std::fs::read(child_dir.join(path)).unwrap(),
                    content.as_bytes(),
                    "{path}, mode={mode}"
                );
                if path != "existing.txt" {
                    assert!(
                        !parent.dir().join(path).exists(),
                        "a sub-agent's work lands only through the host"
                    );
                }
            }
            let copy = Vcs::open(&child_dir, Identity::default())
                .await
                .unwrap()
                .working_copy()
                .await
                .unwrap();
            assert_eq!(copy.is_committed(), successful, "mode={mode}");
            assert_eq!(
                std::fs::read(parent.dir().join("existing.txt")).unwrap(),
                b"base\n"
            );
            let child_bookmark = project
                .head(&Head::Run(RunId(child_run.id.as_str().into())))
                .unwrap();
            assert!(child_bookmark.is_some());
            if !successful {
                assert!(records.iter().any(|(_, body)| {
                    serde_json::from_str::<
                        serde_json::Value,
                    >(body)
                    .unwrap()["finalization_error"]
                        .is_string()
                }));
                assert_eq!(
                    project
                        .head(&Head::Run(outcome.run.clone()))
                        .unwrap()
                        .unwrap(),
                    project.trunk().unwrap()
                );
            }
            child.assert_exhausted();
            llm.assert_exhausted();
        });
    }
}

/// jj's native snapshot omits new oversized files. A clean `@` alone
/// therefore cannot authorize workspace removal. Retain boundary witnesses.
#[test]
fn oversized_untracked_child_files_are_not_discarded() {
    for extra in [1, 32] {
        let home = tempfile::tempdir().unwrap();
        let project = project_with(home.path(), &[("existing.txt", "base\n")]);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let store = tau_store_sqlite::memory().await.unwrap();
            let parent = caller_in(&project, "parent");
            let llm = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call(
                        "spawn",
                        json!({"task": "write the large file"}),
                    )
                })
                .turn(|t| t.text("caller done"));
            let content = "x".repeat(1_048_576 + extra);
            let expected = content.clone();
            let child = ScriptedModel::new().turn(move |t| {
                t.tool_call(
                    "write",
                    json!({"path": "large.txt", "content": content}),
                )
            });
            let script = child.clone();
            let (agents, mut ends) = heard_sub_agents();
            let agent =
                coder_in(llm.clone(), parent.as_ref()).tool(Spawn::new(
                    parent.clone(),
                    agents,
                    &notes(),
                    &[],
                    ready_child(move |workspace, _| {
                        Ok(coder_in(script.clone(), workspace.as_ref())
                            .limits(Limits::default().max_turns(1)))
                    }),
                ));
            let outcome = agent.run("delegate", &store).await.unwrap();
            let (_, ending) = ends.recv().await.unwrap();
            let Ending::Retained { workspace: dir, .. } = ending else {
                panic!("{ending:?}")
            };
            assert_eq!(
                std::fs::read(dir.join("large.txt")).unwrap(),
                expected.as_bytes()
            );
            assert!(!parent.dir().join("large.txt").exists());
            let children = store.subagents(&outcome.run.0).await.unwrap();
            assert!(
                project
                    .head(&Head::Run(RunId(children[0].id.as_str().into())))
                    .unwrap()
                    .is_some()
            );
            child.assert_exhausted();
            llm.assert_exhausted();
        });
    }
}
