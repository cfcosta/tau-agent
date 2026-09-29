//! The pure parts of fast compaction: the token estimate, the ledger and
//! the fitted state, over generated transcripts.

use std::collections::BTreeSet;

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use tau_ai::message::{AssistantBlock, InputBlock, Message};
use tau_fast_compaction::{
    Action,
    Decision,
    Ledger,
    state::{self, estimate_tokens, fit_state},
};
use tau_testing::generators;

/// Pinned values, computed with pi's own `estimateTokens` in Node (its
/// float sum lands on the exact count for each): a word of letters is
/// one token per six letters, rounded up; digits half a token each;
/// other non-space characters nine tenths.
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

/// The estimate, counted exactly in tenths of a token: a word of ASCII
/// letters is ten tenths per started six letters, a digit five, any
/// other non-space character nine; the total rounds up to whole tokens.
/// Ten punctuation characters are 9 tokens, where pi's float sum gives
/// 10.
#[hegel::test]
#[hegel::explicit_test_case(text = String::from(".........."))]
fn the_token_estimate_counts_exact_tenths(tc: TestCase) {
    let text: String = tc.draw(gs::text().alphabet("ab19.,- \n"));
    let mut tenths = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_ascii_alphabetic() {
            let mut len = 1usize;
            while chars.next_if(char::is_ascii_alphabetic).is_some() {
                len += 1;
            }
            tenths += 10 * len.div_ceil(6);
        } else if c.is_ascii_digit() {
            tenths += 5;
        } else if !c.is_whitespace() {
            tenths += 9;
        }
    }
    assert_eq!(estimate_tokens(&text), tenths.div_ceil(10));
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

fn action() -> impl PrintableGenerator<Action> {
    // Action is tau's own type, so its drawn values print through Debug.
    action_unprinted().print_as_debug()
}

#[hegel::composite]
fn action_unprinted(tc: &TestCase) -> Action {
    tc.draw(
        gs::sampled_from(vec![
            Action::Keep,
            Action::DropResult,
            Action::DropCall,
        ])
        .print_as_debug(),
    )
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
fn ledger_for(transcript: Vec<Message>) -> impl PrintableGenerator<Ledger> {
    // Ledger is tau's own type, so its drawn values print through Debug.
    ledger_for_unprinted(transcript).print_as_debug()
}

#[hegel::composite]
fn ledger_for_unprinted(tc: &TestCase, transcript: Vec<Message>) -> Ledger {
    Ledger::from_decisions(call_ids(&transcript).into_iter().filter_map(|id| {
        tc.draw(gs::booleans())
            .then(|| decision(&id, tc.draw(action())))
    }))
}

/// The note a cut result starts with.
const TRUNCATED: &str = "[fast-compaction truncated ";

/// Text of exactly a drawn length, up to `max_chars` characters, some of
/// them several bytes long.
#[hegel::composite]
fn sized_text(tc: &TestCase, max_chars: usize) -> String {
    let chars = tc.draw(gs::integers::<usize>().max_value(max_chars));
    tc.draw(
        gs::text()
            .alphabet("ab \n.é日🦀")
            .min_size(chars)
            .max_size(chars),
    )
}

/// A tool result's content: texts of up to 3000 characters, sometimes
/// one already cut, and sometimes images.
fn result_content() -> impl PrintableGenerator<Vec<InputBlock>> {
    // InputBlock is tau's own type, so its drawn values print through Debug.
    result_content_unprinted().print_as_debug()
}

#[hegel::composite]
fn result_content_unprinted(tc: &TestCase) -> Vec<InputBlock> {
    let blocks = tc.draw(gs::integers::<usize>().max_value(3));
    (0..blocks)
        .map(|_| match tc.draw(gs::integers::<u8>().max_value(5)) {
            0 => InputBlock::Image(tc.draw(generators::image_content())),
            1 => text_block(format!("{TRUNCATED}{}", tc.draw(sized_text(200)))),
            _ => text_block(tc.draw(sized_text(3000))),
        })
        .collect()
}

fn text_block(text: String) -> InputBlock {
    InputBlock::Text(tau_ai::message::TextContent {
        text,
        text_signature: None,
    })
}

/// A drawn transcript whose tool results hold [`result_content`].
#[hegel::composite]
fn transcript_with_long_results(tc: &TestCase) -> Vec<Message> {
    tc.draw(generators::transcript())
        .into_iter()
        .map(|message| match message {
            Message::ToolResult(mut result) => {
                result.content = tc.draw(result_content());
                Message::ToolResult(result)
            }
            other => other,
        })
        .collect()
}

/// What a ledger makes of a transcript, stated from its documentation:
/// calls dropped with their results; an assistant message losing a call
/// and left with neither text nor calls goes; a dropped result whose
/// text is not already cut and either holds an image or runs past
/// `head + 120` characters becomes its first `head` characters, a
/// newline when there are any, and a note of what was cut; everything
/// else stays as it is.
fn expected_apply(
    tc: &TestCase,
    ledger: &Ledger,
    transcript: &[Message],
    head: usize,
) -> Vec<Message> {
    let action = |id: &str| {
        ledger
            .decisions()
            .find(|d| d.call_id == id)
            .map_or(Action::Keep, |d| d.action)
    };
    let mut surviving = BTreeSet::new();
    let mut expected = Vec::new();
    for message in transcript {
        match message {
            Message::Assistant(assistant) => {
                let content: Vec<AssistantBlock> = assistant
                    .content
                    .iter()
                    .filter(|b| {
                        !matches!(b, AssistantBlock::ToolCall(call)
                            if action(&call.id) == Action::DropCall)
                    })
                    .cloned()
                    .collect();
                let lost_a_call = content.len() < assistant.content.len();
                let left_with_words = content.iter().any(|b| {
                    matches!(
                        b,
                        AssistantBlock::Text(_) | AssistantBlock::ToolCall(_)
                    )
                });
                if lost_a_call && !left_with_words {
                    tc.event("assistant message dropped");
                    continue;
                }
                if !lost_a_call
                    && assistant
                        .content
                        .iter()
                        .all(|b| matches!(b, AssistantBlock::Thinking(_)))
                {
                    tc.event("thinking-only message kept");
                }
                for block in &content {
                    if let AssistantBlock::ToolCall(call) = block {
                        surviving.insert(call.id.clone());
                    }
                }
                let mut assistant = assistant.clone();
                assistant.content = content;
                expected.push(Message::Assistant(assistant));
            }
            Message::ToolResult(result) => {
                if !surviving.contains(&result.tool_call_id) {
                    continue;
                }
                match action(&result.tool_call_id) {
                    Action::DropCall => {}
                    Action::Keep => expected.push(message.clone()),
                    Action::DropResult => {
                        let text = state::block_text(&result.content);
                        let chars = text.chars().count();
                        let images = result
                            .content
                            .iter()
                            .any(|b| matches!(b, InputBlock::Image(_)));
                        if text.contains(TRUNCATED) {
                            tc.event("already cut");
                            expected.push(message.clone());
                        } else if images || chars > head + 120 {
                            tc.event("cut");
                            let kept: String =
                                text.chars().take(head).collect();
                            let newline = if head > 0 { "\n" } else { "" };
                            let error =
                                if result.is_error { " (error)" } else { "" };
                            let note = format!(
                                "{TRUNCATED}{} chars of this tool result{error}; re-run the tool if needed]",
                                chars.saturating_sub(head)
                            );
                            let mut cut = result.clone();
                            cut.content = vec![text_block(format!(
                                "{kept}{newline}{note}"
                            ))];
                            expected.push(Message::ToolResult(cut));
                        } else {
                            tc.event("kept-short");
                            expected.push(message.clone());
                        }
                    }
                }
            }
            Message::User(_) => expected.push(message.clone()),
        }
    }
    expected
}

/// Applying a ledger keeps every surviving call with its result right
/// after its message, drops a dropped call with its result, leaves every
/// user message as it was and in order, and is idempotent. An empty
/// ledger changes nothing. What comes out is exactly [`expected_apply`]:
/// long or image-holding dropped results are cut to their head and a
/// note, short ones and ones already cut stay, and an unpruned
/// thinking-only message stays.
#[hegel::test(test_cases = 300)]
fn the_ledger_keeps_the_transcript_well_formed(tc: TestCase) {
    let transcript = tc.draw(transcript_with_long_results().print_as_debug());
    let ledger = tc.draw(ledger_for(transcript.clone()));
    let head = tc.draw(gs::integers::<usize>().max_value(200));
    let applied = ledger.apply(&transcript, head);
    assert_eq!(applied, expected_apply(&tc, &ledger, &transcript, head));

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
}

/// Decisions only escalate: after any merges, each call holds the first
/// of its decisions with the harshest action (a milder or equal later
/// one changes nothing, keep_call included), and calls come in id order.
#[hegel::test(test_cases = 200)]
fn decisions_only_escalate(tc: TestCase) {
    let decisions: Vec<Vec<Decision>> = tc.draw(
        gs::vecs(gs::vecs(decision_drawn().print_as_debug()).max_size(4))
            .min_size(1)
            .max_size(4),
    );
    let mut batches = decisions.iter().cloned();
    let mut ledger = Ledger::from_decisions(batches.next().unwrap());
    for batch in batches {
        ledger.merge(batch);
    }
    let mut model: std::collections::BTreeMap<String, Decision> =
        std::collections::BTreeMap::new();
    for decision in decisions.into_iter().flatten() {
        match model.get(&decision.call_id) {
            Some(held) if held.action >= decision.action => {
                tc.event("not escalated");
            }
            _ => {
                model.insert(decision.call_id.clone(), decision);
            }
        }
    }
    let now: Vec<Decision> = ledger.decisions().cloned().collect();
    assert_eq!(now, model.into_values().collect::<Vec<_>>());
}

/// A decision for one of three calls, with a drawn action and keep_call.
#[hegel::composite]
fn decision_drawn(tc: &TestCase) -> Decision {
    let id = tc.draw(gs::sampled_from(vec!["a", "b", "c"]));
    Decision {
        keep_call: tc.draw(gs::sampled_from(vec![0.0, 0.25, 0.5, 1.0])),
        ..decision(id, tc.draw(action()))
    }
}

/// Text of up to 1500 characters with no capital letters, so it never
/// says an output marker, long enough to be abridged.
#[hegel::composite]
fn long_text(tc: &TestCase) -> String {
    let chars = tc.draw(gs::integers::<usize>().max_value(1500));
    tc.draw(
        gs::text()
            .alphabet("abcdefgh ij\n.,1é")
            .min_size(chars)
            .max_size(chars),
    )
}

/// A drawn transcript whose prompts and assistant texts are
/// [`long_text`], whose tool inputs each hold one, and whose results each say only a marker that nothing
/// else in the transcript can say.
#[hegel::composite]
fn transcript_with_long_texts(tc: &TestCase) -> Vec<Message> {
    tc.draw(generators::transcript())
        .into_iter()
        .enumerate()
        .map(|(index, message)| match message {
            Message::User(mut user) => {
                user.content =
                    tau_ai::message::UserContent::Text(tc.draw(long_text()));
                Message::User(user)
            }
            Message::Assistant(mut assistant) => {
                for block in &mut assistant.content {
                    match block {
                        AssistantBlock::Text(text) => {
                            text.text = tc.draw(long_text());
                        }
                        AssistantBlock::ToolCall(call) => {
                            call.arguments.insert(
                                "long".into(),
                                tc.draw(long_text()).into(),
                            );
                        }
                        AssistantBlock::Thinking(_) => {}
                    }
                }
                Message::Assistant(assistant)
            }
            Message::ToolResult(mut result) => {
                result.content =
                    vec![text_block(format!("OUTPUT-MARKER-{index}"))];
                Message::ToolResult(result)
            }
        })
        .collect()
}

/// The size a failed fit reports: `~N tokens after shrinking`.
fn reported_size(message: String) -> usize {
    message
        .split_once('~')
        .and_then(|(_, rest)| rest.split_once(' '))
        .and_then(|(tokens, _)| tokens.parse().ok())
        .unwrap_or_else(|| panic!("no size in {message:?}"))
}

/// A fitted state stays within its budget, its size is its base plus
/// each history entry's estimate and a separator, it never loses a
/// pinned message that has text or calls (the first one, and the recent
/// ones), and it never holds a tool's output. Fitting fails only when
/// the fully shrunk state is over budget: given the size it reports as
/// its budget, the same history fits.
#[hegel::test(test_cases = 200)]
fn the_state_fits_and_keeps_pinned_messages(tc: TestCase) {
    let transcript = tc.draw(transcript_with_long_texts().print_as_debug());
    let preserve = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let entries = state::entries(&transcript);
    let calls = state::collect_calls(&entries, preserve);
    // Budgets from just under the fully shrunk size, the one fitting
    // reports when nothing fits, to just past the unshrunk one, so every
    // stage is reached.
    let unshrunk = fit_state(&entries, &calls, None, usize::MAX, preserve)
        .unwrap()
        .tokens;
    let shrunk = reported_size(
        fit_state(&entries, &calls, None, 0, preserve).unwrap_err(),
    );
    let budget = tc.draw(
        gs::integers::<usize>()
            .min_value(shrunk.saturating_sub(10))
            .max_value(unshrunk + 10),
    );
    let fitted = match fit_state(&entries, &calls, None, budget, preserve) {
        Ok(fitted) => fitted,
        Err(message) => {
            tc.event("over budget");
            let reported = reported_size(message);
            assert!(reported > budget);
            assert_eq!(reported, shrunk, "failing never shrinks less");
            let fitted =
                fit_state(&entries, &calls, None, reported, preserve).unwrap();
            assert_eq!(fitted.tokens, reported);
            return;
        }
    };
    tc.event(fitted.stage);
    assert!(fitted.tokens <= budget, "{} > {budget}", fitted.tokens);
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
    assert_eq!(fitted.tokens, base + history);
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
