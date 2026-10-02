//! Bash grants are exercised through a scripted agent and the real nested
//! artifact reader; no model provider or live evaluation is involved.

#![cfg(unix)]

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tau_agent::{
    agent::Agent,
    error::ToolError,
    plugin::Plugin,
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_artifacts::{Bytes, Quotas};
use tau_store::Store;
// Process and Store I/O require a real-time runtime with I/O enabled.
use tau_testing::{block_on_io as block_on, scripted::ScriptedModel};
use tau_tools::{
    bash::Bash,
    path::Root,
    plugin::{CodingTools, Tool},
};

fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        json!({
            "type":"object", "properties": {
                "command":{"type":"string"}, "timeout":{"type":"number"},
                "id":{"type":"string"}, "page":{"type":"boolean"}
            }, "additionalProperties":false
        })
    })
}

#[derive(Clone)]
struct Fixture {
    coding: CodingTools,
    bash: Arc<Bash>,
    captured: Arc<Mutex<Vec<Value>>>,
}

impl Fixture {
    fn new(root: Root, storage: Option<Bytes>, terminal: bool) -> Self {
        let mut coding = CodingTools::new(root.clone()).without(Tool::Bash);
        let mut bash = Bash::new(root);
        #[cfg(feature = "terminal")]
        {
            bash = bash.with_terminal(terminal);
        }
        #[cfg(not(feature = "terminal"))]
        let _ = terminal;
        if let Some(bytes) = storage {
            coding = coding.with_artifacts(bytes.clone());
            bash = bash.with_artifacts(bytes);
        }
        Self {
            coding,
            bash: Arc::new(bash),
            captured: Arc::default(),
        }
    }

    fn last(&self) -> Value {
        self.captured.lock().unwrap().last().unwrap().clone()
    }
}

impl Plugin for Fixture {
    fn name(&self) -> &str {
        tau_tools::ui::NAME
    }
    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        let mut tools = self.coding.tools();
        tools.push(self.bash.clone());
        tools.push(Arc::new(Capture(self.captured.clone())));
        tools
    }
}

struct Capture(Arc<Mutex<Vec<Value>>>);

#[async_trait]
impl AgentTool for Capture {
    fn name(&self) -> &str {
        "capture_bash"
    }
    fn description(&self) -> &str {
        "Captures bash and artifact ranges for the fixture."
    }
    fn parameters(&self) -> &Value {
        schema()
    }
    async fn call(
        &self,
        args: Value,
        ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let mut captured = json!({});
        if let Some(command) = args["command"].as_str() {
            let mut bash_args = json!({"command": command});
            if args["timeout"].is_number() {
                bash_args["timeout"] = args["timeout"].clone();
            }
            let result = ctx.call("bash", bash_args).await;
            let (output, failed) = match result {
                Ok(output) => (output, false),
                Err(ToolError::Output(output)) => (*output, true),
                Err(error) => return Err(error),
            };
            captured["bash"] = output.structured.unwrap();
            captured["failed"] = json!(failed);
            captured["display"] = json!(output.content);
        }
        let id = args["id"]
            .as_str()
            .or_else(|| captured["bash"]["artifact"]["id"].as_str())
            .map(str::to_owned);
        if let Some(id) = id {
            let mut offset = 0_u64;
            let mut joined = Vec::new();
            let mut pages = Vec::new();
            loop {
                match ctx.call("artifact_read", json!({"id":id,"offset":offset,"limit":4096,"encoding":"base64"})).await {
                    Ok(output) => {
                        let page = output.structured.unwrap();
                        joined.extend(STANDARD.decode(page["data"].as_str().unwrap()).unwrap());
                        let next = page["next_offset"].as_u64();
                        pages.push(page);
                        match next { Some(next) => offset = next, None => break }
                    }
                    Err(error) => { captured["read_error"] = json!(error.to_string()); break; }
                }
            }
            if captured["read_error"].is_null() {
                captured["bytes"] = json!(STANDARD.encode(joined));
                captured["pages"] = json!(pages);
            }
        }
        self.0.lock().unwrap().push(captured);
        Ok(ToolOutput::text("captured"))
    }
}

fn model(args: Value) -> ScriptedModel {
    ScriptedModel::new()
        .turn(move |turn| turn.tool_call("capture_bash", args))
        .turn(|turn| turn.text("done"))
}

fn decoded(captured: &Value) -> Vec<u8> {
    STANDARD
        .decode(captured["bytes"].as_str().unwrap())
        .unwrap()
}

fn objects(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir.join("artifacts/objects"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.path().extension().is_some_and(|ext| ext == "blob")
        })
        .count()
}

#[test]
fn full_output_pages_cover_first_middle_and_last_markers_in_both_modes() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Bytes::new(dir.path().join("artifacts"), Quotas::default())
                .unwrap();
        let mut expected = String::new();
        for row in 0..2405 {
            let marker = match row {
                0 => "FIRST",
                1202 => "MIDDLE",
                2404 => "LAST",
                _ => "row",
            };
            expected
                .push_str(&format!("{row:04}|{marker}|{}\n", "x".repeat(65)));
        }
        assert!(expected.len() > 150 * 1024);
        std::fs::write(dir.path().join("log.txt"), &expected).unwrap();
        let store = Store::memory().await.unwrap();
        for terminal in [false, true] {
            let fixture = Fixture::new(
                Root::new(dir.path()),
                Some(storage.clone()),
                terminal,
            );
            Agent::new(model(json!({"command":"cat log.txt"})))
                .plugin(fixture.clone())
                .run("large output", &store)
                .await
                .unwrap();
            let got = fixture.last();
            assert_eq!(got["bash"]["source_complete"], true);
            assert_eq!(got["bash"]["status"], "exited");
            assert_eq!(got["bash"]["truncated"], true);
            assert_eq!(got["bash"]["artifact"]["size_bytes"], expected.len());
            let restored = decoded(&got);
            assert_eq!(restored, expected.as_bytes());
            let restored = std::str::from_utf8(&restored).unwrap();
            let lines: Vec<&str> = restored.lines().collect();
            assert!(lines[0].contains("FIRST"));
            assert!(lines[1202].contains("MIDDLE"));
            assert!(lines[2404].contains("LAST"));
            assert!(got["pages"].as_array().unwrap().len() > 30);
            let spill = got["bash"]["spill_path"].as_str().unwrap();
            assert_eq!(std::fs::read(spill).unwrap(), expected.as_bytes());
            assert!(std::path::Path::new(spill).exists());
        }
    });
}

#[test]
fn complete_single_binary_byte_is_read_back_unchanged() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Bytes::new(dir.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture = Fixture::new(Root::new(dir.path()), Some(storage), false);
        let store = Store::memory().await.unwrap();
        Agent::new(model(json!({"command":"printf '\\377'"})))
            .plugin(fixture.clone())
            .run("binary output", &store)
            .await
            .unwrap();
        let got = fixture.last();
        assert_eq!(got["bash"]["source_complete"], true);
        assert_eq!(got["bash"]["artifact"]["size_bytes"], 1);
        assert_eq!(got["bash"]["artifact_error"], Value::Null);
        assert_eq!(decoded(&got), [0xff]);
    });
}

#[test]
fn empty_command_has_no_artifact_or_artifact_error() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Bytes::new(dir.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture = Fixture::new(Root::new(dir.path()), Some(storage), false);
        let store = Store::memory().await.unwrap();
        Agent::new(model(json!({"command":""})))
            .plugin(fixture.clone())
            .run("empty output", &store)
            .await
            .unwrap();
        let got = fixture.last();
        assert_eq!(got["failed"], false);
        assert_eq!(got["bash"]["source_complete"], true);
        assert_eq!(got["bash"]["total_bytes"], 0);
        assert_eq!(got["bash"]["artifact"], Value::Null);
        assert_eq!(got["bash"]["artifact_error"], Value::Null);
        assert_eq!(objects(dir.path()), 0);
    });
}

#[test]
fn nonzero_and_timeout_preserve_status_while_publishing_observed_output() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Bytes::new(dir.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture = Fixture::new(Root::new(dir.path()), Some(storage), false);
        let store = Store::memory().await.unwrap();
        Agent::new(model(
            json!({"command":"printf 'before failure\\n'; exit 7"}),
        ))
        .plugin(fixture.clone())
        .run("nonzero", &store)
        .await
        .unwrap();
        let nonzero = fixture.last();
        assert_eq!(nonzero["failed"], true);
        assert_eq!(nonzero["bash"]["exit_code"], 7);
        assert_eq!(nonzero["bash"]["source_complete"], true);
        assert_eq!(decoded(&nonzero), b"before failure\n");

        Agent::new(model(json!({"command":"printf 'before timeout\\n'; sleep 2", "timeout":0.2})))
            .plugin(fixture.clone()).run("timeout", &store).await.unwrap();
        let timeout = fixture.last();
        assert_eq!(timeout["failed"], true);
        assert_eq!(timeout["bash"]["status"], "timed_out");
        assert_eq!(timeout["bash"]["exit_code"], Value::Null);
        assert_eq!(timeout["bash"]["source_complete"], true);
        assert_eq!(decoded(&timeout), b"before timeout\n");
    });
}

#[test]
fn storage_failure_has_no_grant_and_grants_follow_run_scope() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage = Bytes::new(
            dir.path().join("artifacts"),
            Quotas {
                max_artifact_bytes: 4,
                total_bytes: 64,
            },
        )
        .unwrap();
        let fixture =
            Fixture::new(Root::new(dir.path()), Some(storage.clone()), false);
        let store = Store::memory().await.unwrap();
        Agent::new(model(json!({"command":"printf 'too long'"})))
            .plugin(fixture.clone())
            .run("quota", &store)
            .await
            .unwrap();
        let rejected = fixture.last();
        assert!(rejected["bash"]["artifact"].is_null());
        assert!(
            rejected["bash"]["artifact_error"]
                .as_str()
                .unwrap()
                .contains("quota")
        );
        assert_eq!(objects(dir.path()), 0);

        let generous =
            Bytes::new(dir.path().join("generous"), Quotas::default()).unwrap();
        let fixture =
            Fixture::new(Root::new(dir.path()), Some(generous), false);
        let parent = Agent::new(model(json!({"command":"printf 'scoped'"})))
            .plugin(fixture.clone())
            .run("parent", &store)
            .await
            .unwrap();
        let id = fixture.last()["bash"]["artifact"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        Agent::new(model(json!({"id":id})))
            .plugin(fixture.clone())
            .run("unrelated", &store)
            .await
            .unwrap();
        assert!(
            fixture.last()["read_error"]
                .as_str()
                .unwrap()
                .contains("not granted")
        );
        Agent::new(model(json!({"id":id})))
            .plugin(fixture.clone())
            .fork(&parent.checkpoint())
            .run("child", &store)
            .await
            .unwrap();
        assert_eq!(decoded(&fixture.last()), b"scoped");

        let absent = Fixture::new(Root::new(dir.path()), None, false);
        Agent::new(model(json!({"command":"printf 'direct'"})))
            .plugin(absent.clone())
            .run("no storage", &store)
            .await
            .unwrap();
        assert!(absent.last()["bash"]["artifact"].is_null());
        assert!(
            absent.last()["bash"]["artifact_error"]
                .as_str()
                .unwrap()
                .contains("storage is unavailable")
        );
    });
}

#[cfg(feature = "terminal")]
#[test]
fn terminal_artifact_contains_plain_text_not_vt_replay() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let storage =
            Bytes::new(dir.path().join("artifacts"), Quotas::default())
                .unwrap();
        let fixture = Fixture::new(Root::new(dir.path()), Some(storage), true);
        let store = Store::memory().await.unwrap();
        Agent::new(model(
            json!({"command":"printf '\\033[31mRED\\033[0m\\n'"}),
        ))
        .plugin(fixture.clone())
        .run("colored", &store)
        .await
        .unwrap();
        let got = fixture.last();
        assert_eq!(decoded(&got), b"RED\n");
        assert_eq!(got["bash"]["source_complete"], true);
    });
}
