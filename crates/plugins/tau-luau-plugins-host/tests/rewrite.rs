//! A Luau plugin's rewrite, as built from what it answers: whatever it
//! keeps and inserts, the rewrite is one the loop takes, and holds what
//! it asked for.

use hegel::{TestCase, generators as gs};
use serde_json::{Map, Value, json};
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
use tau_luau_plugins_host::rewrite::{rebuild, summary};

fn user(text: &str) -> Message {
    Message::User(UserMessage {
        content: UserContent::Text(text.into()),
        timestamp: 0,
    })
}

fn assistant(text: &str, calls: &[String]) -> Message {
    let mut content = vec![AssistantBlock::Text(TextContent {
        text: text.into(),
        text_signature: None,
    })];
    content.extend(calls.iter().map(|id| {
        AssistantBlock::ToolCall(ToolCall {
            id: id.clone(),
            name: "exec_command".into(),
            arguments: Map::new(),
        })
    }));
    Message::Assistant(AssistantMessage {
        content,
        model: "m".into(),
        response_id: None,
        usage: Usage::default(),
        stop_reason: if calls.is_empty() {
            StopReason::Stop
        } else {
            StopReason::ToolUse
        },
        error_message: None,
        timestamp: 0,
    })
}

fn result(id: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "exec_command".into(),
        content: vec![InputBlock::Text(TextContent {
            text: format!("output of {id}"),
            text_signature: None,
        })],
        details: None,
        is_error: false,
        timestamp: 0,
    })
}

/// A transcript of turns, from its shape: for each turn, the calls of
/// each step; then whether the person said one more thing. Each turn is
/// the person's message, then answers that call tools and their results,
/// and a plain answer.
fn transcript(turns: &[Vec<usize>], more: bool) -> Vec<Message> {
    let mut messages = Vec::new();
    let mut next = 0;
    for (turn, steps) in turns.iter().enumerate() {
        messages.push(user(&format!("ask {turn}")));
        for calls in steps {
            let ids: Vec<String> = (0..*calls)
                .map(|_| {
                    next += 1;
                    format!("c{next}")
                })
                .collect();
            messages.push(assistant("working", &ids));
            messages.extend(ids.iter().map(|id| result(id)));
        }
        messages.push(assistant("done", &[]));
    }
    if more {
        messages.push(user("one more"));
    }
    messages
}

/// The loop's own rules for a rewrite (`check_rewrite`): not empty, ends
/// with the transcript's last message, every call followed by its
/// results before anything else.
fn takes(transcript: &[Message], messages: &[Message]) -> Result<(), String> {
    if messages.last() != transcript.last() {
        return Err("does not end with the last message".into());
    }
    let mut open: Vec<String> = Vec::new();
    for message in messages {
        match message {
            Message::Assistant(assistant) => {
                if !open.is_empty() {
                    return Err(format!("{open:?} left without results"));
                }
                open = assistant.tool_calls().map(|c| c.id.clone()).collect();
            }
            Message::ToolResult(result) => {
                let Some(at) =
                    open.iter().position(|id| *id == result.tool_call_id)
                else {
                    return Err(format!("{} has no call", result.tool_call_id));
                };
                open.remove(at);
            }
            Message::User(_) => {
                if !open.is_empty() {
                    return Err(format!("{open:?} left without results"));
                }
            }
        }
    }
    if open.is_empty() {
        Ok(())
    } else {
        Err(format!("{open:?} left without results"))
    }
}

#[hegel::test(test_cases = 300)]
fn a_rewrite_is_one_the_loop_takes(tc: TestCase) {
    let turns: Vec<Vec<usize>> = tc.draw(
        gs::vecs(
            gs::vecs(gs::integers::<usize>().min_value(1).max_value(3))
                .max_size(2),
        )
        .min_size(1)
        .max_size(4),
    );
    let transcript = transcript(&turns, tc.draw(gs::booleans()));
    let count = transcript.len();
    let keep: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(1).max_value(count))
            .max_size(count),
    );
    let inserts: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(1).max_value(count))
            .max_size(3),
    );
    let answer = json!({
        "keep": keep,
        "insert": inserts
            .iter()
            .enumerate()
            .map(|(n, before)| json!({ "before": before, "text": format!("note {n}") }))
            .collect::<Vec<_>>(),
        "drops_conversation": true,
    });
    let rewrite = rebuild(&transcript, &answer, "p", 0).unwrap();
    let Some(rewrite) = rewrite else {
        // Nothing asked that the transcript does not already hold.
        assert!(inserts.is_empty());
        return;
    };
    takes(&transcript, &rewrite.messages).unwrap();
    // What it asked to keep is kept, and each note is there once.
    for index in &keep {
        assert!(rewrite.messages.contains(&transcript[index - 1]));
    }
    for n in 0..inserts.len() {
        let note = user(&format!("note {n}"));
        assert_eq!(rewrite.messages.iter().filter(|m| **m == note).count(), 1);
    }
    assert!(rewrite.drops_conversation);
    assert_eq!(rewrite.details["plugin"], "p");
}

/// Keeping nothing but what it must still keeps the last message, with
/// the call it answers.
#[test]
fn the_last_message_and_its_call_always_stay() {
    let transcript = vec![
        user("go"),
        assistant("one", &["a".into()]),
        result("a"),
        assistant("two", &["b".into()]),
        result("b"),
    ];
    let rewrite = rebuild(&transcript, &json!({ "keep": [] }), "p", 0)
        .unwrap()
        .unwrap();
    assert_eq!(rewrite.messages, transcript[3..]);
}

/// A place outside the transcript is refused, with why.
#[test]
fn a_message_that_is_not_there_is_refused() {
    let transcript = vec![user("go"), assistant("done", &[])];
    let error =
        rebuild(&transcript, &json!({ "keep": [3] }), "p", 0).unwrap_err();
    assert!(error.contains("not a message"), "{error}");
}

/// The summary names each message by its place, gives its text whole
/// and says how long it is (ADR 0050, gap B).
#[test]
fn the_summary_names_and_sizes_each_message() {
    let long = "x".repeat(5_000);
    let transcript =
        vec![user(&long), assistant("calls", &["a".into()]), result("a")];
    let summary = summary(&transcript);
    assert_eq!(summary[0]["index"], 1);
    assert_eq!(summary[0]["role"], "user");
    assert_eq!(summary[0]["size"], 5_000);
    assert_eq!(summary[0]["text"], long);
    assert_eq!(
        summary[1]["calls"],
        json!([{ "id": "a", "name": "exec_command", "args": {}, "order": [], "json": "{}" }])
    );
    assert_eq!(summary[2]["call"], "a");
    assert_eq!(summary[2]["ok"], Value::Bool(true));
}
