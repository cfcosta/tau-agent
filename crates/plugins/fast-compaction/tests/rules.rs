//! The pure parts of fast compaction: the token estimate, the ledger and
//! the fitted state, over generated transcripts.

use std::collections::BTreeSet;

use hegel::{TestCase, generators as gs};
use tau_ai::message::{AssistantBlock, InputBlock, Message};
use tau_fast_compaction::{
    Action,
    Decision,
    Ledger,
    state::{self, estimate_tokens, fit_state},
};
use tau_testing::generators;

/// Pinned values, computed with pi's own `estimateTokens` in Node: a word
/// of letters is one token per six letters, rounded up; digits half a
/// token each; other non-space characters nine tenths.
#[test]
fn the_token_estimate_matches_pis() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("hello world"), 2);
    assert_eq!(estimate_tokens("abcdefgh"), 2);
    assert_eq!(estimate_tokens("abcdefghijklm"), 3);
    assert_eq!(estimate_tokens("12345"), 3);
    assert_eq!(estimate_tokens("a.b"), 3);
    assert_eq!(estimate_tokens("{\"k\":1}"), 6);
    assert_eq!(estimate_tokens("  \n\t "), 0);
    assert_eq!(estimate_tokens("é"), 1);
}

/// The tool call ids of a transcript, in order.
fn call_ids(transcript: &[Message]) -> Vec<String> {
    transcript
        .iter()
        .flat_map(|message| match message {
            Message::Assistant(assistant) => assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    AssistantBlock::ToolCall(call) => Some(call.id.clone()),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[hegel::composite]
fn action(tc: TestCase) -> Action {
    tc.draw(gs::sampled_from(vec![
        Action::Keep,
        Action::DropResult,
        Action::DropCall,
    ]))
}

fn decision(call_id: &str, action: Action) -> Decision {
    Decision {
        call_id: call_id.to_owned(),
        tool: "t".into(),
        action,
        keep_call: 0.5,
        keep_result: 0.5,
    }
}

/// A ledger with a drawn action for some of the transcript's calls.
#[hegel::composite]
fn ledger_for(tc: TestCase, transcript: Vec<Message>) -> Ledger {
    Ledger::from_decisions(call_ids(&transcript).into_iter().filter_map(|id| {
        tc.draw(gs::booleans())
            .then(|| decision(&id, tc.draw(action())))
    }))
}

/// Applying a ledger keeps every surviving call with its result right
/// after its message, drops a dropped call with its result, leaves every
/// user message as it was and in order, and is idempotent. An empty
/// ledger changes nothing.
#[hegel::test(test_cases = 300)]
fn the_ledger_keeps_the_transcript_well_formed(tc: TestCase) {
    let transcript = tc.draw(generators::transcript());
    let ledger = tc.draw(ledger_for(transcript.clone()));
    let head = tc.draw(gs::integers::<usize>().max_value(400));
    let applied = ledger.apply(&transcript, head);

    assert_eq!(Ledger::default().apply(&transcript, head), transcript);
    assert_eq!(ledger.apply(&applied, head), applied, "idempotent");

    let users = |t: &[Message]| -> Vec<Message> {
        t.iter()
            .filter(|m| matches!(m, Message::User(_)))
            .cloned()
            .collect()
    };
    assert_eq!(users(&applied), users(&transcript));

    let dropped: BTreeSet<String> = ledger
        .decisions()
        .filter(|d| d.action == Action::DropCall)
        .map(|d| d.call_id.clone())
        .collect();
    let surviving = call_ids(&applied);
    assert!(surviving.iter().all(|id| !dropped.contains(id)));
    // Every surviving call's result follows its message, in order.
    for (index, message) in applied.iter().enumerate() {
        let Message::Assistant(assistant) = message else {
            continue;
        };
        let calls: Vec<&str> = assistant
            .content
            .iter()
            .filter_map(|b| match b {
                AssistantBlock::ToolCall(call) => Some(call.id.as_str()),
                _ => None,
            })
            .collect();
        let results: Vec<&str> = applied[index + 1..]
            .iter()
            .take_while(|m| matches!(m, Message::ToolResult(_)))
            .map(|m| match m {
                Message::ToolResult(r) => r.tool_call_id.as_str(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(results, calls, "results after message {index}");
    }
    // A dropped result is never longer than the original.
    for message in &applied {
        if let Message::ToolResult(result) = message
            && let Some(original) = transcript.iter().find_map(|m| match m {
                Message::ToolResult(r)
                    if r.tool_call_id == result.tool_call_id =>
                {
                    Some(r)
                }
                _ => None,
            })
        {
            let text = |content: &[InputBlock]| {
                state::block_text(content).chars().count()
            };
            if result.content != original.content {
                assert!(text(&result.content) <= text(&original.content) + 120);
            }
        }
    }
}

/// Decisions only escalate: merging a milder decision for a call changes
/// nothing, and a harsher one replaces it.
#[hegel::test(test_cases = 200)]
fn decisions_only_escalate(tc: TestCase) {
    let first = tc.draw(action());
    let second = tc.draw(action());
    let mut ledger = Ledger::from_decisions([decision("c", first)]);
    ledger.merge([decision("c", second)]);
    let now: Vec<Action> = ledger.decisions().map(|d| d.action).collect();
    assert_eq!(now, [first.max(second)]);
}

/// A fitted state stays within its budget, never loses a pinned message
/// that has text or calls (the first one, and the recent ones), and
/// never holds a tool's output.
#[hegel::test(test_cases = 200)]
fn the_state_fits_and_keeps_pinned_messages(tc: TestCase) {
    // Each result says only a marker that nothing else in the transcript
    // can say, so finding one in the state means an output leaked.
    let transcript: Vec<Message> = tc
        .draw(generators::transcript())
        .into_iter()
        .enumerate()
        .map(|(index, message)| match message {
            Message::ToolResult(mut result) => {
                result.content =
                    vec![InputBlock::Text(tau_ai::message::TextContent {
                        text: format!("OUTPUT-MARKER-{index}"),
                        text_signature: None,
                    })];
                Message::ToolResult(result)
            }
            other => other,
        })
        .collect();
    let preserve = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let budget = tc.draw(gs::integers::<usize>().min_value(50).max_value(4000));
    let entries = state::entries(&transcript);
    let calls = state::collect_calls(&entries, preserve);
    let Ok(fitted) = fit_state(&entries, &calls, None, budget, preserve) else {
        return;
    };
    assert!(fitted.tokens <= budget, "{} > {budget}", fitted.tokens);
    let kept: BTreeSet<usize> =
        fitted.state.history.iter().map(|e| e.i).collect();
    let call_messages: BTreeSet<usize> =
        calls.iter().map(|c| c.call_index).collect();
    for (index, entry) in entries.iter().enumerate() {
        let has_content =
            !entry.text.trim().is_empty() || call_messages.contains(&index);
        if state::is_pinned(index, entries.len(), preserve) && has_content {
            assert!(kept.contains(&index), "pinned message {index} left out");
        }
    }
    let json = serde_json::to_string(&fitted.state).unwrap();
    assert!(
        !json.contains("OUTPUT-MARKER-"),
        "a tool output reached the state"
    );
}

/// A short transcript: prompts, an answer, a tool result, a blank and a
/// long prompt.
fn sample() -> Vec<Message> {
    use tau_ai::message::{
        AssistantMessage,
        TextContent,
        ToolResultMessage,
        UserContent,
        UserMessage,
    };
    let user = |text: &str| {
        Message::User(UserMessage {
            content: UserContent::Text(text.into()),
            timestamp: 0,
        })
    };
    let assistant = Message::Assistant(AssistantMessage {
        content: vec![AssistantBlock::Text(TextContent {
            text: "not a prompt".into(),
            text_signature: None,
        })],
        api: String::new(),
        provider: String::new(),
        model: String::new(),
        response_id: None,
        usage: Default::default(),
        stop_reason: tau_ai::message::StopReason::Stop,
        error_message: None,
        timestamp: 0,
    });
    let result = Message::ToolResult(ToolResultMessage {
        tool_call_id: "c".into(),
        tool_name: "t".into(),
        content: vec![InputBlock::Text(TextContent {
            text: "not a prompt either".into(),
            text_signature: None,
        })],
        details: None,
        is_error: false,
        timestamp: 0,
    });
    vec![
        user("one"),
        assistant,
        user("two"),
        result,
        user("   "),
        user("three"),
        user(&"y".repeat(600)),
    ]
}

/// Without a goal, the state's is the user's last three prompts, each cut
/// to 500 characters; tool results and the assistant's words are not
/// prompts, and neither is a blank.
#[test]
fn the_goal_is_the_last_prompts() {
    let entries = state::entries(&sample());
    let goal = format!("two\nthree\n{}…", "y".repeat(499));
    assert_eq!(state::goal_from(&entries), goal);
    let fitted = fit_state(&entries, &[], None, 25_000, 1).unwrap();
    assert_eq!(fitted.state.goal, goal);
}

/// The state's size is its base (context and goal) plus each history
/// entry's estimate and one for its separator.
#[test]
fn the_state_size_adds_up() {
    let entries = state::entries(&sample());
    let fitted = fit_state(&entries, &[], Some("goal"), 25_000, 1).unwrap();
    let base = estimate_tokens(
        &serde_json::to_string(&state::State {
            history: Vec::new(),
            ..fitted.state.clone()
        })
        .unwrap(),
    );
    let history: usize = fitted
        .state
        .history
        .iter()
        .map(|entry| {
            estimate_tokens(&serde_json::to_string(entry).unwrap()) + 1
        })
        .sum();
    assert_eq!(fitted.state.history.len(), 5);
    assert_eq!(fitted.tokens, base + history);
}

/// With room to spare, the state is the whole history.
#[test]
fn a_small_history_fits_in_full() {
    let entries = state::entries(&[]);
    let fitted = fit_state(&entries, &[], Some("ship it"), 25_000, 6).unwrap();
    assert_eq!(fitted.stage, "full");
    assert_eq!(fitted.state.goal, "ship it");
    assert!(fitted.state.history.is_empty());
}
