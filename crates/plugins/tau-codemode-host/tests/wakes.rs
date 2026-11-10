//! A script making many host calls is woken for each one's answer
//! however the call ends: at once, from the blocking pool, or after a
//! yield. An interrupt yield inside mlua's wrapper of an async call once
//! spent the call's wake-up, and the script slept to its deadline: of 200
//! rounds of 1,000 calls, 156 stalled from the blocking pool and all 200
//! after a yield.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::json;
use tau_codemode::store::Snapshot;
use tau_codemode_host::{
    CancellationToken,
    Host,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    options,
    run,
};
use tau_testing::block_on_io;

use crate::common::tool;

/// `now` answers at once, `blocking` after a task on the blocking pool
/// (as a file read does), `yield` after a yield to the runtime.
struct Answers;

#[async_trait]
impl Host for Answers {
    fn tools(&self) -> Vec<ToolEntry> {
        vec![
            tool("now", "Answers at once."),
            tool("blocking", "Answers from the blocking pool."),
            tool("yield", "Answers after a yield."),
        ]
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        match call.name.as_str() {
            "blocking" => {
                tokio::task::spawn_blocking(|| ()).await.unwrap();
            }
            "yield" => tokio::task::yield_now().await,
            _ => {}
        }
        Ok(ToolReply {
            value: json!(1),
            error: None,
            usage: None,
            usage_complete: None,
        })
    }
}

/// Runs `calls` calls of `name`, with `spin` steps of Luau between each
/// (so the interrupt yields between some of them), to the end; how long
/// it took, or `None` past two seconds.
fn round(name: &str, calls: usize, spin: usize) -> Option<Duration> {
    let code = format!(
        "-- @options: {{\"timeout_ms\": 2000}}\nlocal n = 0\nfor i = 1, {calls} do\n  \
         n += tools.{name}()\n  local x = 0\n  for j = 1, {spin} do x += j \
         end\nend\nreturn n"
    );
    let source = options::parse(&code).expect("the source parses");
    let started = std::time::Instant::now();
    let outcome = block_on_io(run(
        Arc::new(Answers),
        Request {
            call_id: "call_1".into(),
            source,
            store: Snapshot::new(),
            cancel: CancellationToken::new(),
            inputs: Vec::new(),
        },
    ));
    match &outcome.failure {
        None => Some(started.elapsed()),
        Some(failure) if failure.head().contains("timed out") => None,
        Some(failure) => panic!("the script failed: {}", failure.head()),
    }
}

#[test]
fn many_calls_are_each_woken() {
    for name in ["now", "blocking", "yield"] {
        for spin in [0, 7, 300] {
            for n in 0..10 {
                assert!(
                    round(name, 1_000, spin).is_some(),
                    "round {n} of `{name}` with {spin} steps between stalled"
                );
            }
        }
    }
}
