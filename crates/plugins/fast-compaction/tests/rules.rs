//! The pure parts of fast compaction: the token estimate, the ledger and
//! the history stage's requests, over generated transcripts.

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
    decide,
    plan::REQUEST_OVERHEAD_TOKENS,
    state::{self, estimate_state_tokens, estimate_tokens},
};
use tau_testing::generators;

/// Pinned values of jev-pruner's `estimateTokens` and
/// `estimateStateTokens` (`src/jev.ts`), worked by hand from its
/// calibration: a word of ASCII letters is one token per six letters,
/// rounded up; a digit half a token (a whole one in a state); any other
/// character that is not a space nine tenths, per UTF-16 unit. They
/// replace the values pinned to pi's estimate, which counted a crab as
/// one character and a state's digits as half a token.
#[test]
fn the_token_estimate_matches_jev_pruners() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("hello world"), 2);
    assert_eq!(estimate_tokens("abcdefgh"), 2);
    assert_eq!(estimate_tokens("abcdefghijklm"), 3);
    assert_eq!(estimate_tokens("12345"), 3);
    assert_eq!(estimate_tokens("a.b"), 3);
    assert_eq!(estimate_tokens("{\"k\":1}"), 6);
    assert_eq!(estimate_tokens("  \n\t "), 0);
    assert_eq!(estimate_tokens("é"), 1);
    // Two UTF-16 units: 1.8.
    assert_eq!(estimate_tokens("🦀"), 2);
    // JavaScript's `\s` has U+FEFF and not U+0085.
    assert_eq!(estimate_tokens("\u{feff}"), 0);
    assert_eq!(estimate_tokens("\u{85}"), 1);
    // Exact tenths: ten symbols are 9, where a float sum gives 10.
    assert_eq!(estimate_tokens(".........."), 9);
    assert_eq!(estimate_state_tokens("12345"), 5);
    assert_eq!(estimate_state_tokens("{\"k\":1}"), 7);
}

/// What JavaScript's `\s` matches, as UTF-16 units.
fn js_space_unit(unit: u16) -> bool {
    matches!(
        unit,
        0x09..=0x0d
            | 0x20
            | 0xa0
            | 0x1680
            | 0x2000..=0x200a
            | 0x2028
            | 0x2029
            | 0x202f
            | 0x205f
            | 0x3000
            | 0xfeff
    )
}

/// jev-pruner's estimate, as its regular expression reads the text:
/// over UTF-16 units, a run of ASCII letters, a run of digits, or one
/// unit that is none of those nor a space. Summed in floats, in tenths,
/// where every sum is a whole number and exact, and rounded up once.
fn reference(text: &str, digit: f64) -> usize {
    let units: Vec<u16> = text.encode_utf16().collect();
    let letter = |u: u16| u < 128 && (u as u8).is_ascii_alphabetic();
    let number = |u: u16| u < 128 && (u as u8).is_ascii_digit();
    let mut tenths = 0.0f64;
    let mut at = 0;
    while at < units.len() {
        let unit = units[at];
        if letter(unit) {
            let start = at;
            while at < units.len() && letter(units[at]) {
                at += 1;
            }
            tenths += 10.0 * (1.0 + ((at - start - 1) / 6) as f64);
            continue;
        }
        if number(unit) {
            tenths += digit;
        } else if !js_space_unit(unit) {
            tenths += 9.0;
        }
        at += 1;
    }
    (tenths / 10.0).ceil() as usize
}

/// Both estimates match an independent reading of jev-pruner's: letters,
/// digits, spaces of both kinds, and characters of one and two UTF-16
/// units.
#[hegel::test(test_cases = 300)]
fn the_token_estimate_matches_a_reference(tc: TestCase) {
    let text: String = tc.draw(
        gs::text()
            .alphabet("abcdefgXYZ0189.,-{}\" \n\t\u{a0}\u{85}\u{feff}é日🦀"),
    );
    if text.contains('🦀') {
        tc.event("a character of two units");
    }
    assert_eq!(estimate_tokens(&text), reference(&text, 5.0));
    assert_eq!(estimate_state_tokens(&text), reference(&text, 10.0));
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
        archive: None,
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
/// says an output marker.
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
/// [`long_text`], whose tool inputs each hold one, and whose results
/// each say only a marker that nothing else in the transcript can say.
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

/// The history stage's requests, over any history and budgets: every
/// state fits `max_state_tokens` and every request, with its questions,
/// `max_request_tokens`. When the history fits whole, one state holds
/// all of it; otherwise each state holds a segment, lists the calls it
/// asks about, and the segments hold the whole history once, in order.
/// No tool output ever reaches a state. Every unpinned call is asked
/// about once per segment, unless it is too big to fit beside some
/// (then it stays), and no pinned one is.
#[hegel::test(test_cases = 200)]
fn the_history_stage_asks_about_every_call_in_every_segment(tc: TestCase) {
    let transcript = tc.draw(transcript_with_long_texts().print_as_debug());
    let preserve = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let max_state =
        tc.draw(gs::integers::<usize>().min_value(1500).max_value(8000));
    let max_request = max_state
        + tc.draw(gs::integers::<usize>().min_value(400).max_value(3000));
    let entries = state::entries(&transcript);
    let calls = state::collect_calls(&entries, preserve);
    let goal = "fix the bug in a.rs";
    let planned =
        decide::requests(&entries, &calls, Some(goal), max_state, max_request)
            .unwrap_or_else(|error| panic!("{error}"));
    // Every call fits beside every segment when it takes no more than
    // half of what the state's fixed part leaves.
    let fixed = estimate_state_tokens(
        &serde_json::to_string(&state::State {
            context: state::STATE_CONTEXT,
            goal: goal.into(),
            history: Vec::new(),
            calls: Some(Vec::new()),
        })
        .unwrap(),
    );
    let fits_anywhere = |call: &state::Call| {
        let record = serde_json::to_string(&state::call_record(call)).unwrap();
        (state::tenths(&record, state::STATE_DIGIT_TENTHS) + 9).div_ceil(10)
            <= max_state.saturating_sub(fixed).div_ceil(2)
    };
    tc.event(if planned.segments == 1 {
        "whole"
    } else {
        "segments"
    });
    let mut asked: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); calls.len()];
    let mut by_segment: Vec<Option<Vec<usize>>> = vec![None; planned.segments];
    for (request, segment, calls_asked) in &planned.requests {
        let state = serde_json::to_string(&request.state).unwrap();
        let questions = serde_json::to_string(&request.questions).unwrap();
        let state_tokens = estimate_state_tokens(&state);
        assert!(state_tokens <= max_state, "{state_tokens} > {max_state}");
        assert!(
            state_tokens
                + estimate_state_tokens(&questions)
                + REQUEST_OVERHEAD_TOKENS
                <= max_request
        );
        assert!(
            !state.contains("OUTPUT-MARKER-"),
            "a tool output reached the state"
        );
        assert_eq!(request.state["calls"].is_null(), planned.segments == 1);
        for &call in calls_asked {
            assert!(!calls[call].pinned);
            assert!(asked[call].insert(*segment), "asked twice in a segment");
            if planned.segments > 1 {
                let listed = request.state["calls"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|listed| listed["id"] == calls[call].id.as_str());
                assert!(listed, "{} asked but not listed", calls[call].id);
            }
        }
        let mut seen: Vec<usize> = request.state["history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| record["i"].as_u64().unwrap() as usize)
            .collect();
        seen.dedup();
        by_segment[*segment] = Some(seen);
    }
    for (index, call) in calls.iter().enumerate() {
        if call.pinned {
            assert!(asked[index].is_empty());
        } else if fits_anywhere(call) {
            assert_eq!(asked[index].len(), planned.segments, "{}", call.id);
        } else {
            tc.event("a call too big for some segments");
        }
    }
    if by_segment.iter().all(Option::is_some) {
        let whole: Vec<usize> = state::history_records(&entries, &calls)
            .iter()
            .map(|record| record.i)
            .collect();
        let mut all: Vec<usize> =
            by_segment.into_iter().flatten().flatten().collect();
        all.dedup();
        assert_eq!(all, whole);
    }
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
    assert_eq!(state::goal_or_prompts(None, &entries), goal);
    assert_eq!(state::goal_or_prompts(Some(""), &entries), goal);
    assert_eq!(state::goal_or_prompts(Some("ship it"), &entries), "ship it");
}

/// The history keeps every message with text or calls, whole: the long
/// prompt is not abridged, and the blank one and the result are left
/// out.
#[test]
fn the_history_is_whole() {
    let entries = state::entries(&sample());
    let history = state::history_records(&entries, &[]);
    let indices: Vec<usize> = history.iter().map(|record| record.i).collect();
    assert_eq!(indices, [0, 1, 2, 5, 6]);
    assert_eq!(history[4].text, "y".repeat(600));
}

/// A fresh directory for one test case's archives.
fn scratch_dir(name: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CASE: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "tau-fast-compaction-{name}-{}-{}",
        std::process::id(),
        CASE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every result a ledger cuts names, in its note, an archive that holds
/// the result's text exactly, readable only by its owner; each archive
/// belongs to one cut result, and a second assignment finds nothing new
/// to archive.
#[hegel::test(test_cases = 100)]
fn a_cut_result_names_an_archive_holding_it(tc: TestCase) {
    use std::os::unix::fs::PermissionsExt as _;
    let transcript = tc.draw(transcript_with_long_results().print_as_debug());
    let mut ledger = tc.draw(ledger_for(transcript.clone()));
    let head = tc.draw(gs::integers::<usize>().max_value(200));
    let dir = scratch_dir("ledger");
    let archives = ledger.assign_archives(&transcript, head, &dir);
    for (path, text) in &archives {
        tau_fast_compaction::archive::write(path, text).unwrap();
    }
    assert!(ledger.assign_archives(&transcript, head, &dir).is_empty());
    let applied = ledger.apply(&transcript, head);
    let originals: std::collections::BTreeMap<String, String> = transcript
        .iter()
        .filter_map(|message| match message {
            Message::ToolResult(result) => Some((
                result.tool_call_id.clone(),
                state::block_text(&result.content),
            )),
            _ => None,
        })
        .collect();
    let mut named = BTreeSet::new();
    for message in &applied {
        let Message::ToolResult(result) = message else {
            continue;
        };
        let text = state::block_text(&result.content);
        let original = &originals[&result.tool_call_id];
        if &text == original {
            continue;
        }
        tc.event("cut");
        let (_, rest) = text
            .rsplit_once("; full result: ")
            .unwrap_or_else(|| panic!("no archive named in {text:?}"));
        let (path, _) =
            rest.split_once(" (read or grep it if needed)]").unwrap();
        assert_eq!(&std::fs::read_to_string(path).unwrap(), original);
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        assert!(named.insert(path.to_owned()), "{path} named twice");
    }
    assert_eq!(named.len(), archives.len());
    std::fs::remove_dir_all(&dir).unwrap();
}
