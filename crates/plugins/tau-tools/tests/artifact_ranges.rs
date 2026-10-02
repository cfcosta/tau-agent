//! Grant fixtures publish through a real CodingTools plugin context. The
//! model never supplies metadata or a filesystem path to artifact_read.

#![cfg(unix)]

use std::{
    io::Cursor,
    sync::{Arc, Mutex, OnceLock},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    plugin::Plugin,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_artifacts::{Bytes, Quotas};
use tau_store::{Entry, Store};
use tau_testing::{block_on, scripted::ScriptedModel};
use tau_tools::{
    artifact_grant::{
        ArtifactMetadata,
        ArtifactRecord,
        fold_grants,
        publish_artifact,
    },
    path::Root,
    plugin::CodingTools,
};

fn empty_args_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| json!({"type":"object","properties":{},"additionalProperties":false}))
}

fn read_request_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type":"object",
            "properties":{"request":{
                "type":"object",
                "properties":{
                    "id":{"type":"string"},
                    "offset":{"type":"integer"},
                    "limit":{"type":"integer"},
                    "encoding":{"type":"string"},
                    "digest":{"type":"string"},
                    "size_bytes":{"type":"integer"}
                },
                "required":["id"],
                "additionalProperties":false
            }},
            "required":["request"],
            "additionalProperties":false
        })
    })
}

#[derive(Clone)]
struct FixtureCodingTools {
    coding: CodingTools,
    bytes: Bytes,
    published_artifacts: Arc<Mutex<Vec<ArtifactMetadata>>>,
    read_results: Arc<Mutex<Vec<Value>>>,
    name: &'static str,
}

impl FixtureCodingTools {
    fn new(root: Root, bytes: Bytes) -> Self {
        Self {
            coding: CodingTools::new(root).with_artifacts(bytes.clone()),
            bytes,
            published_artifacts: Arc::default(),
            read_results: Arc::default(),
            name: tau_tools::ui::NAME,
        }
    }
}

impl Plugin for FixtureCodingTools {
    fn name(&self) -> &str {
        self.name
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let mut tools = self.coding.tools();
        tools.push(Arc::new(PublishFixture(self.clone())));
        tools.push(Arc::new(ReadRangeFixture(self.read_results.clone())));
        tools
    }
}

struct PublishFixture(FixtureCodingTools);

#[async_trait]
impl AgentTool for PublishFixture {
    fn name(&self) -> &str {
        "publish_fixture"
    }
    fn description(&self) -> &str {
        "Publishes the trusted test bytes."
    }
    fn parameters(&self) -> &Value {
        empty_args_schema()
    }

    async fn call(
        &self,
        _args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let grant = publish_artifact(
            &self.0.bytes,
            Cursor::new(b"grant bytes\0".to_vec()),
            "trusted fixture",
            &ctx,
        )
        .await?;
        self.0
            .published_artifacts
            .lock()
            .unwrap()
            .push(grant.artifact);
        Ok(ToolOutput::text("published"))
    }
}

struct ReadRangeFixture(Arc<Mutex<Vec<Value>>>);

#[async_trait]
impl AgentTool for ReadRangeFixture {
    fn name(&self) -> &str {
        "read_range_fixture"
    }
    fn description(&self) -> &str {
        "Calls the nested range reader."
    }
    fn parameters(&self) -> &Value {
        read_request_schema()
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let answer =
            match ctx.call("artifact_read", args["request"].clone()).await {
                Ok(output) => json!({"ok": output.structured}),
                Err(error) => json!({"error": error.to_string()}),
            };
        self.0.lock().unwrap().push(answer.clone());
        Ok(ToolOutput::text(answer.to_string()))
    }
}

fn publish_model() -> ScriptedModel {
    ScriptedModel::new()
        .turn(|t| t.tool_call("publish_fixture", json!({})))
        .turn(|t| t.text("done"))
}

fn read_model(request: Value) -> ScriptedModel {
    ScriptedModel::new()
        .turn(move |t| {
            t.tool_call("read_range_fixture", json!({"request":request}))
        })
        .turn(|t| t.text("done"))
}

fn last_read_result(fixture: &FixtureCodingTools) -> Value {
    fixture
        .read_results
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap()
}

#[test]
fn grants_follow_fork_cutoffs_but_not_other_conversations() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let store = Store::memory().await.unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes.clone());
        let parent = Agent::new(publish_model())
            .plugin(fixture.clone())
            .run("publish", &store)
            .await
            .unwrap();
        let first = fixture.published_artifacts.lock().unwrap()[0].clone();
        let request =
            json!({"id":first.id,"encoding":"base64","offset":0,"limit":4});

        Agent::new(read_model(request.clone()))
            .plugin(fixture.clone())
            .run("unrelated conversation", &store)
            .await
            .unwrap();
        assert!(
            last_read_result(&fixture)["error"]
                .as_str()
                .unwrap()
                .contains("not granted")
        );

        Agent::new(read_model(request.clone()))
            .plugin(fixture.clone())
            .fork(&parent.checkpoint())
            .run("child", &store)
            .await
            .unwrap();
        let received = last_read_result(&fixture);
        let range = &received["ok"];
        assert_eq!(range["id"], first.id);
        assert_eq!(range["offset"], 0);
        assert_eq!(range["next_offset"], 4);
        assert_eq!(range["data"], "Z3Jhbg==");
        assert_eq!(range["complete"], false);

        let later = Agent::new(publish_model())
            .plugin(fixture.clone())
            .resume(&parent.run)
            .run("later parent grant", &store)
            .await
            .unwrap();
        let second = fixture.published_artifacts.lock().unwrap()[1].clone();
        assert_ne!(first.id, second.id);
        let guessed = json!({"id":second.id,"digest":second.digest,"size_bytes":second.size_bytes});
        Agent::new(read_model(json!({"id":second.id})))
            .plugin(fixture.clone())
            .fork(&parent.checkpoint())
            .run("old checkpoint", &store)
            .await
            .unwrap();
        assert!(
            last_read_result(&fixture)["error"]
                .as_str()
                .unwrap()
                .contains("not granted")
        );
        Agent::new(read_model(guessed))
            .plugin(fixture.clone())
            .run("guessed metadata", &store)
            .await
            .unwrap();
        assert!(last_read_result(&fixture)["error"].is_string());

        let reopened =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let restarted =
            FixtureCodingTools::new(Root::new(directory.path()), reopened);
        Agent::new(read_model(json!({"id":second.id})))
            .plugin(restarted.clone())
            .fork(&later.checkpoint())
            .run("after storage restart", &store)
            .await
            .unwrap();
        assert_eq!(
            last_read_result(&restarted)["ok"]["data"],
            "grant bytes\u{0}"
        );
        assert_eq!(last_read_result(&restarted)["ok"]["complete"], true);

        let records: Vec<Value> = store
            .records(&parent.run.0, tau_tools::ui::NAME)
            .await
            .unwrap()
            .into_iter()
            .map(|body| serde_json::from_str(&body).unwrap())
            .collect();
        let grants = fold_grants(&records).unwrap();
        assert_eq!(grants[&first.id].owner_run_id, parent.run.0.as_ref());
        assert_eq!(grants[&first.id].source, "trusted fixture");
        assert_eq!(records[0]["kind"], "artifact_grant");
        let _: ArtifactRecord =
            serde_json::from_value(records[0].clone()).unwrap();
    });
}

#[test]
fn publication_requires_coding_tools_and_owned_storage_is_explicit() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes.clone());
        let detached = PublishFixture(fixture.clone())
            .call(json!({}), ToolCtx::detached())
            .await;
        assert!(detached.unwrap_err().to_string().contains("plugin context"));

        let wrong = FixtureCodingTools {
            name: "other",
            ..fixture.clone()
        };
        let store = Store::memory().await.unwrap();
        let wrong_run = Agent::new(publish_model())
            .plugin(wrong.clone())
            .run("wrong plugin", &store)
            .await
            .unwrap();
        assert!(wrong.published_artifacts.lock().unwrap().is_empty());
        assert!(
            store
                .records(&wrong_run.run.0, "other")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(store.transcript(&wrong_run.run.0).await.unwrap().iter().any(|entry| {
            matches!(entry, Entry::Message { body, .. } if body.contains("requires CodingTools"))
        }));

        let no_storage = FixtureCodingTools {
            coding: CodingTools::new(Root::new(directory.path())),
            ..fixture.clone()
        };
        Agent::new(read_model(json!({"id":"any"})))
            .plugin(no_storage.clone())
            .run("storage absent", &store)
            .await
            .unwrap();
        assert!(
            last_read_result(&no_storage)["error"]
                .as_str()
                .unwrap()
                .contains("storage is unavailable")
        );
    });
}
