//! Grant fixtures publish through a real CodingTools plugin context. The
//! model never supplies metadata or a filesystem path to artifact_read.

#![cfg(unix)]

use std::{
    io::Cursor,
    sync::{Arc, Mutex, OnceLock},
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hegel::{TestCase, generators as gs};
use image::{DynamicImage, ImageFormat, RgbImage};
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
    read::Read,
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

fn file_request_schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type":"object",
            "properties": {
                "request": {
                    "type":"object",
                    "properties": {
                        "path":{"type":"string"},
                        "offset":{"type":"integer"},
                        "limit":{"type":"integer"}
                    },
                    "required":["path"],
                    "additionalProperties":false
                },
                "page":{"type":"boolean"}
            },
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
    file_results: Arc<Mutex<Vec<Value>>>,
    name: &'static str,
}

impl FixtureCodingTools {
    fn new(root: Root, bytes: Bytes) -> Self {
        Self {
            coding: CodingTools::new(root).with_artifacts(bytes.clone()),
            bytes,
            published_artifacts: Arc::default(),
            read_results: Arc::default(),
            file_results: Arc::default(),
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
        tools.push(Arc::new(ReadFileFixture(self.file_results.clone())));
        tools.push(Arc::new(DropPublicationFixture(self.bytes.clone())));
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

struct ReadFileFixture(Arc<Mutex<Vec<Value>>>);

#[async_trait]
impl AgentTool for ReadFileFixture {
    fn name(&self) -> &str {
        "read_file_fixture"
    }
    fn description(&self) -> &str {
        "Captures a read and optionally pages its artifact."
    }
    fn parameters(&self) -> &Value {
        file_request_schema()
    }

    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let captured = match ctx.call("read", args["request"].clone()).await {
            Ok(output) => {
                let mut captured = json!({
                    "structured": output.structured,
                    "content": output.content,
                });
                if args["page"] == true {
                    let id = captured["structured"]["artifact"]["id"]
                        .as_str()
                        .ok_or("read did not publish an artifact")?;
                    let mut offset = 0_u64;
                    let mut pages = Vec::new();
                    loop {
                        let page = ctx.call("artifact_read", json!({
                            "id": id, "offset": offset, "limit": 7, "encoding": "utf8"
                        })).await?.structured.ok_or("range has no structured output")?;
                        let next = page["next_offset"].as_u64();
                        pages.push(page);
                        match next {
                            Some(next) => offset = next,
                            None => break,
                        }
                    }
                    captured["pages"] = json!(pages);
                }
                captured
            }
            Err(error) => json!({"error": error.to_string()}),
        };
        self.0.lock().unwrap().push(captured);
        Ok(ToolOutput::text("captured"))
    }
}

struct DropPublicationFixture(Bytes);

struct BlockingReader {
    started: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
    finished: Option<tokio::sync::oneshot::Sender<()>>,
}

impl std::io::Read for BlockingReader {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if let Some(started) = self.started.take() {
            let _ = started.send(());
            self.release.recv().unwrap();
            bytes[0] = b'x';
            return Ok(1);
        }
        Ok(0)
    }
}

impl Drop for BlockingReader {
    fn drop(&mut self) {
        if let Some(finished) = self.finished.take() {
            let _ = finished.send(());
        }
    }
}

#[async_trait]
impl AgentTool for DropPublicationFixture {
    fn name(&self) -> &str {
        "drop_publication_fixture"
    }
    fn description(&self) -> &str {
        "Drops publication while its reader blocks."
    }
    fn parameters(&self) -> &Value {
        empty_args_schema()
    }

    async fn call(
        &self,
        _args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
        let reader = BlockingReader {
            started: Some(started_tx),
            release: release_rx,
            finished: Some(finished_tx),
        };
        let mut publication =
            Box::pin(publish_artifact(&self.0, reader, "blocked reader", &ctx));
        tokio::select! {
            result = &mut publication => return result.map(|_| ToolOutput::text("unexpected publication")),
            started = started_rx => started.map_err(ToolError::other)?,
        }
        drop(publication);
        release_tx.send(()).map_err(ToolError::other)?;
        finished_rx.await.map_err(ToolError::other)?;
        Ok(ToolOutput::text("dropped"))
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

fn file_model(request: Value, page: bool) -> ScriptedModel {
    ScriptedModel::new()
        .turn(move |t| {
            t.tool_call(
                "read_file_fixture",
                json!({"request":request,"page":page}),
            )
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

fn published_objects(directory: &std::path::Path) -> usize {
    std::fs::read_dir(directory.join("artifacts/objects"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "blob")
        })
        .count()
}

#[test]
fn read_artifacts_recover_bytes_past_line_and_byte_display_limits() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes);
        let store = Store::memory().await.unwrap();
        let sources = [
            (
                format!("{}\nLINE_AFTER_2000", vec!["row"; 2_005].join("\n")),
                "LINE_AFTER_2000",
            ),
            (
                format!("{}\nBYTE_AFTER_50K", "x".repeat(60_000)),
                "BYTE_AFTER_50K",
            ),
        ];
        for (index, (source, marker)) in sources.into_iter().enumerate() {
            let name = format!("case-{index}.txt");
            std::fs::write(directory.path().join(&name), &source).unwrap();
            let direct = Read::new(Root::new(directory.path()))
                .call(json!({"path":name}), ToolCtx::detached())
                .await
                .unwrap();
            let parent = Agent::new(file_model(json!({"path":name}), false))
                .plugin(fixture.clone())
                .run("read file", &store)
                .await
                .unwrap();
            let captured =
                fixture.file_results.lock().unwrap().last().unwrap().clone();
            assert_eq!(
                captured["content"],
                serde_json::to_value(&direct.content).unwrap()
            );
            let result = &captured["structured"];
            assert_eq!(result["artifact"]["size_bytes"], source.len());
            assert_eq!(
                result["artifact"]["source"],
                directory.path().join(&name).display().to_string()
            );
            assert!(!result["text"].as_str().unwrap().contains(marker));
            let at = source.find(marker).unwrap() as u64;
            let request = json!({"id":result["artifact"]["id"],"offset":at,"limit":marker.len(),"encoding":"utf8"});
            Agent::new(read_model(request.clone()))
                .plugin(fixture.clone())
                .fork(&parent.checkpoint())
                .run("authorized child", &store)
                .await
                .unwrap();
            assert_eq!(last_read_result(&fixture)["ok"]["data"], marker);
            Agent::new(read_model(json!({"id":result["artifact"]["id"],"offset":0,"limit":source.len(),"encoding":"utf8"})))
                .plugin(fixture.clone()).fork(&parent.checkpoint())
                .run("full source", &store).await.unwrap();
            assert_eq!(last_read_result(&fixture)["ok"]["data"], source);
            assert_eq!(last_read_result(&fixture)["ok"]["complete"], true);
            Agent::new(read_model(request))
                .plugin(fixture.clone())
                .run("unrelated child", &store)
                .await
                .unwrap();
            assert!(
                last_read_result(&fixture)["error"]
                    .as_str()
                    .unwrap()
                    .contains("not granted")
            );
        }
    });
}

#[test]
fn binary_read_artifact_preserves_original_bytes() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let original = b"head\0\xff\xc3\nend";
        std::fs::write(directory.path().join("binary.dat"), original).unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes);
        let store = Store::memory().await.unwrap();
        let parent =
            Agent::new(file_model(json!({"path":"binary.dat"}), false))
                .plugin(fixture.clone())
                .run("binary", &store)
                .await
                .unwrap();
        let captured =
            fixture.file_results.lock().unwrap().last().unwrap().clone();
        let id = &captured["structured"]["artifact"]["id"];
        Agent::new(read_model(json!({"id":id,"encoding":"base64"})))
            .plugin(fixture.clone())
            .fork(&parent.checkpoint())
            .run("binary artifact", &store)
            .await
            .unwrap();
        let encoded = last_read_result(&fixture)["ok"]["data"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(STANDARD.decode(encoded).unwrap(), original);
    });
}

#[test]
fn image_read_keeps_direct_image_and_pages_original_file_bytes() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(RgbImage::new(2, 1))
            .write_to(&mut encoded, ImageFormat::Bmp)
            .unwrap();
        let original = encoded.into_inner();
        std::fs::write(directory.path().join("image.bmp"), &original).unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes);
        let store = Store::memory().await.unwrap();
        let parent = Agent::new(file_model(json!({"path":"image.bmp"}), false))
            .plugin(fixture.clone())
            .run("image", &store)
            .await
            .unwrap();
        let captured =
            fixture.file_results.lock().unwrap().last().unwrap().clone();
        assert_eq!(captured["structured"]["kind"], "image");
        assert_eq!(captured["structured"]["content"], captured["content"]);
        let id = &captured["structured"]["artifact"]["id"];
        Agent::new(read_model(json!({"id":id,"encoding":"base64"})))
            .plugin(fixture.clone())
            .fork(&parent.checkpoint())
            .run("original image", &store)
            .await
            .unwrap();
        let encoded = last_read_result(&fixture)["ok"]["data"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(STANDARD.decode(encoded).unwrap(), original);
    });
}

#[test]
fn file_larger_than_default_artifact_quota_reports_failure_without_reference() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(directory.path().join("oversize.dat"))
            .unwrap();
        file.set_len(64 * 1024 * 1024 + 1).unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes);
        let store = Store::memory().await.unwrap();
        let run = Agent::new(file_model(json!({"path":"oversize.dat"}), false))
            .plugin(fixture.clone())
            .run("oversize", &store)
            .await
            .unwrap();
        let result =
            fixture.file_results.lock().unwrap().last().unwrap().clone();
        assert!(result["structured"]["artifact"].is_null());
        assert!(
            result["structured"]["artifact_error"]
                .as_str()
                .unwrap()
                .contains("byte quota")
        );
        assert_eq!(published_objects(directory.path()), 0);
        assert!(
            store
                .records(&run.run.0, tau_tools::ui::NAME)
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn quota_failure_returns_artifact_error_without_a_grant_or_object() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("large.txt"), "x".repeat(80))
            .unwrap();
        let bytes = Bytes::new(
            directory.path().join("artifacts"),
            Quotas {
                max_artifact_bytes: 64,
                total_bytes: 1_024,
            },
        )
        .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes);
        let store = Store::memory().await.unwrap();
        let run = Agent::new(file_model(json!({"path":"large.txt"}), false))
            .plugin(fixture.clone())
            .run("quota", &store)
            .await
            .unwrap();
        let result =
            fixture.file_results.lock().unwrap().last().unwrap().clone();
        assert_eq!(result["structured"]["text"], "x".repeat(80));
        assert!(result["structured"]["artifact"].is_null());
        assert!(
            result["structured"]["artifact_error"]
                .as_str()
                .unwrap()
                .contains("byte quota")
        );
        assert_eq!(published_objects(directory.path()), 0);
        assert!(
            store
                .records(&run.run.0, tau_tools::ui::NAME)
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn dropped_publication_future_cancels_its_blocking_reader_without_a_grant() {
    block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let bytes =
            Bytes::new(directory.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture =
            FixtureCodingTools::new(Root::new(directory.path()), bytes);
        let store = Store::memory().await.unwrap();
        let model = ScriptedModel::new()
            .turn(|t| t.tool_call("drop_publication_fixture", json!({})))
            .turn(|t| t.text("done"));
        let run = Agent::new(model)
            .plugin(fixture)
            .run("drop", &store)
            .await
            .unwrap();
        assert_eq!(published_objects(directory.path()), 0);
        assert!(
            store
                .records(&run.run.0, tau_tools::ui::NAME)
                .await
                .unwrap()
                .is_empty()
        );
    });
}

/// Property inventory: complete Unicode artifact pages equal original file
/// bytes decoded as UTF-8, while source and size equal the resolved input
/// file. The oracle is the original source, never the display string.
/// Generator plan: draw a vector of valid fixed Unicode tokens and join with
/// newlines; shrinking shortens the vector without rejection. The workspace
/// hegel.toml controls case counts. On CI Hegel derandomizes cases, disables
/// its example database, and suppresses TooSlow.
#[hegel::test]
fn unicode_artifact_pages_match_original_file(tc: TestCase) {
    let tokens: Vec<&str> = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            "a", "é", "漢", "😀", "\r", "\0", "Ω",
        ]))
        .min_size(1)
        .max_size(80),
    );
    let source = tokens.join("\n");
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("unicode.txt"), source.as_bytes())
        .unwrap();
    let bytes =
        Bytes::new(directory.path().join("artifacts"), Quotas::default())
            .unwrap();
    let fixture = FixtureCodingTools::new(Root::new(directory.path()), bytes);
    block_on(async {
        let store = Store::memory().await.unwrap();
        Agent::new(file_model(json!({"path":"unicode.txt"}), true))
            .plugin(fixture.clone())
            .run("unicode", &store)
            .await
            .unwrap();
    });
    let captured = fixture.file_results.lock().unwrap().last().unwrap().clone();
    let result = &captured["structured"];
    assert_eq!(result["artifact"]["size_bytes"], source.len());
    assert_eq!(
        result["artifact"]["source"],
        directory.path().join("unicode.txt").display().to_string()
    );
    let mut reconstructed = String::new();
    let mut expected_offset = 0_u64;
    for page in captured["pages"].as_array().unwrap() {
        assert_eq!(page["offset"], expected_offset);
        reconstructed.push_str(page["data"].as_str().unwrap());
        expected_offset += page["data"].as_str().unwrap().len() as u64;
        assert_eq!(
            page["next_offset"],
            if page["eof"] == true {
                Value::Null
            } else {
                json!(expected_offset)
            }
        );
    }
    assert_eq!(reconstructed.as_bytes(), source.as_bytes());
    assert_eq!(expected_offset, source.len() as u64);
}
