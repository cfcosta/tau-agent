//! What building a request's input costs as one run's transcript grows.
//!
//! Each turn adds an assistant message (reasoning, then a tool call) and
//! its tool result of `PAYLOAD` bytes, then converts the whole transcript
//! with the session's `InputCache`, as every request does. Prints the
//! cost of the last turn and of all turns together: if a turn's cost
//! grows with the transcript, the total grows with its square.
//!
//! ```text
//! cargo bench -p tau-ai --bench input
//! TURNS=800 PAYLOAD=16384 cargo bench -p tau-ai --bench input
//! ```

use std::{hint::black_box, time::Instant};

use serde_json::{Map, json};
use tau_ai::{
    message::{
        AssistantBlock,
        AssistantMessage,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        ThinkingContent,
        ToolCall,
        ToolResultMessage,
        Usage,
        UserContent,
        UserMessage,
    },
    responses::input::InputCache,
};

fn env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn turn(n: usize, payload: usize) -> [Message; 2] {
    let id = format!("call_{n}|fc_{n}");
    let mut arguments = Map::new();
    arguments.insert("command".into(), json!(format!("cargo test {n}")));
    let reasoning = json!({
        "type": "reasoning",
        "id": format!("rs_{n}"),
        "summary": [],
        "encrypted_content": "e".repeat(2048),
    });
    let assistant = AssistantMessage {
        content: vec![
            AssistantBlock::Thinking(ThinkingContent {
                thinking: String::new(),
                thinking_signature: Some(reasoning.to_string()),
                redacted: None,
            }),
            AssistantBlock::ToolCall(ToolCall {
                id: id.clone(),
                name: "bash".into(),
                arguments,
            }),
        ],
        model: "gpt-6-sol".into(),
        response_id: Some(format!("resp_{n}")),
        usage: Usage::default(),
        stop_reason: StopReason::ToolUse,
        error_message: None,
        timestamp: 0,
    };
    let result = ToolResultMessage {
        tool_call_id: id,
        tool_name: "bash".into(),
        content: vec![InputBlock::Text(TextContent {
            text: "x".repeat(payload),
            text_signature: None,
        })],
        details: None,
        is_error: false,
        timestamp: 0,
    };
    [Message::Assistant(assistant), Message::ToolResult(result)]
}

fn main() {
    let turns = env("TURNS", 400);
    let payload = env("PAYLOAD", 8192);
    let mut transcript = vec![Message::User(UserMessage {
        content: UserContent::Text("Fix the flaky test.".into()),
        timestamp: 0,
    })];
    let mut cache = InputCache::new();
    let mut total = std::time::Duration::ZERO;
    let mut first = std::time::Duration::ZERO;
    let mut last = std::time::Duration::ZERO;
    for n in 0..turns {
        transcript.extend(turn(n, payload));
        let started = Instant::now();
        black_box(cache.input(black_box(&transcript)));
        let spent = started.elapsed();
        if n == 0 {
            first = spent;
        }
        last = spent;
        total += spent;
    }
    println!(
        "turns={turns} payload={payload}B messages={}",
        transcript.len()
    );
    println!("first turn {first:?}  last turn {last:?}  all turns {total:?}");
}
