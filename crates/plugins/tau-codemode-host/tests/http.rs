//! `http` in a script (ADR 0037): only a run with the `http` tool has
//! it, and a secret reaches the tool as its name, never its value.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::{FakeHost, error, script, texts, tool};
use serde_json::{Value, json};
use tau_codemode::store::Snapshot;
use tau_codemode_host::{
    CancellationToken,
    Host,
    Outcome,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    options,
    run,
};

/// A run with `http`, which answers with what it was asked.
#[derive(Default)]
struct Agent {
    asked: Mutex<Vec<Value>>,
}

#[async_trait]
impl Host for Agent {
    fn tools(&self) -> Vec<ToolEntry> {
        vec![tool("http", "One HTTP request.")]
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        self.asked.lock().unwrap().push(call.args.clone());
        Ok(ToolReply::success(
            json!({ "status": 200, "asked": call.args }),
        ))
    }
}

async fn on(agent: &Arc<Agent>, code: &str) -> Outcome {
    run(
        agent.clone(),
        Request {
            call_id: "call_1".into(),
            source: options::parse(code).expect("the source parses"),
            store: Snapshot::new(),
            cancel: CancellationToken::new(),
        },
    )
    .await
}

#[tokio::test]
async fn a_run_without_the_tool_has_no_http() {
    let outcome =
        script(&Arc::new(FakeHost::default()), "return http == nil").await;
    assert_eq!(texts(&outcome), ["true"]);
}

#[tokio::test]
async fn a_request_names_its_secrets() {
    let agent = Arc::new(Agent::default());
    let outcome = on(
        &agent,
        r#"
        local answer = http.request {
            method = "POST",
            url = "https://api.example.com/items",
            headers = {
                authorization = "Bearer " .. http.secret("github"),
                ["x-key"] = http.secret("other"),
            },
            body = { token = http.secret("other", "k=") },
        }
        return answer.status
        "#,
    )
    .await;
    assert_eq!(outcome.failure, None, "{outcome:?}");
    assert_eq!(texts(&outcome), ["200"]);
    assert_eq!(
        agent.asked.lock().unwrap().as_slice(),
        [json!({
            "method": "POST",
            "url": "https://api.example.com/items",
            "headers": {
                "authorization": { "$secret": "github", "prefix": "Bearer " },
                "x-key": { "$secret": "other" },
            },
            "body": { "token": { "$secret": "other", "prefix": "k=" } },
        })]
    );
}

#[tokio::test]
async fn a_secret_follows_text_only() {
    let agent = Arc::new(Agent::default());
    let outcome = on(&agent, r#"return http.secret("github") .. "!""#).await;
    assert!(
        error(&outcome).contains("can only follow text"),
        "{outcome:?}"
    );
    let outcome = on(&agent, "return http.secret('')").await;
    assert!(error(&outcome).contains("a secret's name"), "{outcome:?}");
}

#[tokio::test]
async fn http_cannot_be_replaced() {
    let agent = Arc::new(Agent::default());
    let outcome = on(&agent, "http.request = nil").await;
    assert!(outcome.failure.is_some());
}
