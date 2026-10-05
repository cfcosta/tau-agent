//! Finalization is a handoff, not permission to discard a child's edits.
//! Generated operation histories carry an independent path -> bytes model.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::collections::BTreeMap;

use async_trait::async_trait;
use common::{coder, project_with};
use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{
    limits::Limits,
    plugin::{FinishedRun, Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
};
use tau_ai::message::Message;
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_vcs::{
    Identity,
    Link,
    RunWorkspace,
    Spawn,
    SubAgents,
    Wait,
    run_workspace::{PLUGIN, bookmark},
};

/// A real jj-lib refusal, not a mocked commit result: tag the already
/// snapshotted working copy immediately before the workspace finishes.
#[derive(Clone)]
struct TagOnFinish(tau_vcs::Vcs);

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
                workspace::Workspace,
            };
            let mut config = StackedConfig::with_defaults();
            let mut user = ConfigLayer::empty(ConfigSource::User);
            user.set_value("user.name", "fixture").unwrap();
            user.set_value("user.email", "fixture@example.invalid")
                .unwrap();
            config.add_layer(user);
            let settings = UserSettings::from_config(config).unwrap();
            let workspace = Workspace::load(
                &settings,
                &dir,
                &default_backend_factories(),
                &default_working_copy_factories(),
            )
            .unwrap();
            let repo =
                pollster::block_on(workspace.repo_loader().load_at_head())
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
fn finalization_lands_exact_bytes_or_keeps_the_child_workspace(tc: TestCase) {
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
            let store = Store::memory().await.unwrap();
            let parent = RunWorkspace::new(
                project.clone().into(),
                "parent",
                Identity::default(),
            )
            .unwrap();
            let llm = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call(
                        "spawn",
                        json!({"task": "write and commit the generated files"}),
                    )
                    .tool_call("wait", json!({}))
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
            let agents = SubAgents::default();
            let agent = coder(llm.clone(), &parent, true)
                .tool(Wait::new(parent.clone(), agents.clone()))
                .tool(Spawn::new(
                    parent.clone(),
                    Identity::default(),
                    agents,
                    &[],
                    ready_child(move |workspace, _| {
                        let agent = if mode == 4 {
                            tau_agent::agent::Agent::new(child_script.clone())
                                .plugin(TagOnFinish(workspace.vcs().clone()))
                                .plugin(tau_tools::plugin::CodingTools::new(
                                    tau_tools::path::Root::new(workspace.dir()),
                                ))
                                .plugin(tau_vcs::VcsPlugin::new(
                                    workspace.vcs().clone(),
                                ))
                                .plugin(workspace)
                        } else {
                            coder(child_script.clone(), &workspace, true)
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
            let requests = llm.requests();
            let result = requests[1]
                .transcript
                .iter()
                .find_map(|message| match message {
                    Message::ToolResult(result)
                        if result.tool_name == "wait" =>
                    {
                        Some(result)
                    }
                    _ => None,
                })
                .unwrap();
            let children = store.subagents(&outcome.run.0).await.unwrap();
            assert_eq!(children.len(), 1);
            let child_run = &children[0];
            let records =
                store.plugin_entries(&child_run.id, PLUGIN).await.unwrap();
            let link = records
                .iter()
                .filter_map(|(_, body)| Link::parse(body))
                .next_back()
                .unwrap();
            let child_dir = project.workspace_dir(&link.workspace);
            let successful = mode == 3;
            assert!(!result.is_error, "mode={mode}, limited={limited}");
            let details = result.details.as_ref().unwrap();
            assert_eq!(
                details["landed"].as_array().unwrap().len(),
                usize::from(successful),
                "mode={mode}, limited={limited}"
            );
            assert_eq!(child_dir.exists(), !successful);
            for (path, content) in &expected {
                let dir = if successful {
                    parent.dir()
                } else {
                    child_dir.clone()
                };
                assert_eq!(
                    std::fs::read(dir.join(path)).unwrap(),
                    content.as_bytes(),
                    "{path}, mode={mode}"
                );
                if !successful && path != "existing.txt" {
                    assert!(
                        !parent.dir().join(path).exists(),
                        "partial child commits must not land"
                    );
                }
            }
            assert_eq!(
                std::fs::read(parent.dir().join("existing.txt")).unwrap(),
                b"base\n"
            );
            let child_bookmark =
                project.bookmark(&format!("tau/{}", child_run.id)).unwrap();
            assert_eq!(child_bookmark.is_some(), !successful);
            let parent_links =
                store.plugin_entries(&outcome.run.0, PLUGIN).await.unwrap();
            let landed = parent_links
                .iter()
                .filter_map(|(_, body)| Link::parse(body))
                .filter(|link| link.from.is_some())
                .count();
            assert_eq!(landed, if successful { committed + 1 } else { 0 });
            if !successful {
                let retained = &details["retained"][0];
                assert_eq!(retained["run"], child_run.id);
                assert_eq!(
                    retained["workspace"],
                    child_dir.display().to_string()
                );
                assert!(records.iter().any(|(_, body)| {
                    serde_json::from_str::<
                        serde_json::Value,
                    >(body)
                    .unwrap()["finalization_error"]
                        .is_string()
                }));
                assert_eq!(
                    project.bookmark(&bookmark(&outcome.run)).unwrap().unwrap(),
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
            let store = Store::memory().await.unwrap();
            let parent = RunWorkspace::new(
                project.clone().into(),
                "parent",
                Identity::default(),
            )
            .unwrap();
            let llm = ScriptedModel::new()
                .turn(|t| {
                    t.tool_call(
                        "spawn",
                        json!({"task": "write the large file"}),
                    )
                    .tool_call("wait", json!({}))
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
            let agents = SubAgents::default();
            let agent = coder(llm.clone(), &parent, true)
                .tool(Wait::new(parent.clone(), agents.clone()))
                .tool(Spawn::new(
                    parent.clone(),
                    Identity::default(),
                    agents,
                    &[],
                    ready_child(move |workspace, _| {
                        Ok(coder(script.clone(), &workspace, true)
                            .limits(Limits::default().max_turns(1)))
                    }),
                ));
            let outcome = agent.run("delegate", &store).await.unwrap();
            let requests = llm.requests();
            let result = requests[1]
                .transcript
                .iter()
                .find_map(|message| match message {
                    Message::ToolResult(result)
                        if result.tool_name == "wait" =>
                    {
                        Some(result)
                    }
                    _ => None,
                })
                .unwrap();
            assert!(!result.is_error);
            let details = &result.details.as_ref().unwrap()["retained"][0];
            let dir =
                std::path::Path::new(details["workspace"].as_str().unwrap());
            assert_eq!(
                std::fs::read(dir.join("large.txt")).unwrap(),
                expected.as_bytes()
            );
            assert!(!parent.dir().join("large.txt").exists());
            let children = store.subagents(&outcome.run.0).await.unwrap();
            assert!(
                project
                    .bookmark(&format!("tau/{}", children[0].id))
                    .unwrap()
                    .is_some()
            );
            child.assert_exhausted();
            llm.assert_exhausted();
        });
    }
}

/// A sub-agent factory that builds its agent at once, as `Spawn` takes
/// one: a future that is ready.
fn ready_child(
    child: impl Fn(
        tau_vcs::RunWorkspace,
        &tau_vcs::sub_agents::ChildModel,
    )
        -> Result<tau_agent::agent::Agent, tau_agent::error::ToolError>
    + Send
    + Sync
    + 'static,
) -> impl Fn(
    tau_vcs::RunWorkspace,
    &tau_vcs::sub_agents::ChildModel,
) -> tau_vcs::sub_agents::ChildFuture
+ Send
+ Sync
+ 'static {
    move |workspace, model| {
        Box::pin(std::future::ready(child(workspace, model)))
    }
}
