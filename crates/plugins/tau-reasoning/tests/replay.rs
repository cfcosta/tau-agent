//! The replay example's run through the plugin: a stored run replays
//! request by request, Jev scoring the message, a lease that holds
//! skipping a request, and a failed tool ending it; each decision says
//! what the stored request went out at.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

#[path = "../examples/replay.rs"]
mod replay;

use std::{collections::BTreeMap, sync::Arc};

use replay::{Entry, plugin, replay};
use serde_json::json;
use tau_ai::message::{
    AssistantBlock,
    AssistantMessage,
    InputBlock,
    Message,
    StopReason,
    TextContent,
    ToolCall,
    ToolResultMessage,
    Usage,
    UserContent,
    UserMessage,
};
use tau_jev::{Answer, fake::FakeJev};
use tau_testing::block_on_io;

/// A Jev that answers each request in turn with a level, sure of it,
/// and a lease.
fn scripted(answers: Vec<(usize, &'static str)>) -> FakeJev {
    let answers = Arc::new(std::sync::Mutex::new(answers.into_iter()));
    FakeJev::new(move |request| {
        let (level, lease) =
            answers.lock().unwrap().next().expect("an answer left");
        let mut answers = BTreeMap::from([(
            "effort".to_owned(),
            Answer::Score {
                score: level as f64,
                probabilities: BTreeMap::from([(level.to_string(), 1.0)]),
                confidence: 0.9,
            },
        )]);
        if request.questions.contains_key("lease") {
            answers.insert(
                "lease".into(),
                Answer::Choice {
                    choice: lease.into(),
                    probabilities: BTreeMap::from([(lease.into(), 1.0)]),
                    confidence: 0.9,
                },
            );
        }
        Ok(tau_jev::fake::response(answers, request))
    })
}

fn text(text: &str) -> TextContent {
    TextContent {
        text: text.into(),
        text_signature: None,
    }
}

fn answer(blocks: Vec<AssistantBlock>) -> Message {
    Message::Assistant(AssistantMessage {
        content: blocks,
        model: "gpt-6-sol".into(),
        response_id: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
}

fn call(id: &str) -> Message {
    answer(vec![AssistantBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "probe".into(),
        arguments: serde_json::Map::new(),
    })])
}

fn result(id: &str, failed: bool) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "probe".into(),
        content: vec![InputBlock::Text(text("ran"))],
        details: None,
        is_error: failed,
        timestamp: 0,
    })
}

#[test]
fn a_stored_run_replays_through_the_plugin() {
    let timeline = vec![
        Entry::Ran(Some("high".into())),
        Entry::Message(Message::User(UserMessage {
            content: UserContent::Text("fix the lane race".into()),
            timestamp: 0,
        })),
        Entry::Message(call("c1")),
        Entry::Message(result("c1", false)),
        Entry::Message(call("c2")),
        Entry::Message(result("c2", true)),
        Entry::Message(answer(vec![AssistantBlock::Text(text("done"))])),
    ];
    let jev = scripted(vec![(1, "tool_chain"), (4, "tool_chain")]);
    let decisions = block_on_io(async {
        let plugin = plugin().await;
        replay(
            &plugin,
            Arc::new(jev),
            "gpt-6-sol",
            json!({ "redecide": true, "threshold": 0.7 }),
            &timeline,
        )
        .await
    });
    let summary: Vec<_> = decisions
        .iter()
        .map(|d| {
            (
                d.request,
                d.step,
                d.recorded.as_deref(),
                d.asked.is_some(),
                d.runs_at.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (1, "user_turn", Some("high"), true, Some("low")),
            (2, "tool_step", Some("high"), false, Some("low")),
            (3, "tool_step", Some("high"), true, Some("xhigh")),
        ]
    );
}
