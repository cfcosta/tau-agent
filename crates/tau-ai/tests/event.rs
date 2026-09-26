//! Event streams and the accumulator (`tau_ai::event`).

use hegel::{TestCase, generators as gs};
use tau_ai::{
    event::{Accumulator, AssistantEvent, DoneReason},
    message::{AssistantBlock, AssistantMessage, StopReason, Usage},
};
use tau_testing::{generators, stream};

fn accumulate(
    events: impl IntoIterator<Item = AssistantEvent>,
) -> AssistantMessage {
    let mut acc = Accumulator::new();
    for event in events {
        acc.push(event).unwrap();
    }
    acc.finish().unwrap()
}

/// Accumulating the stream rendered from a message gives the message
/// back, however its deltas are split.
#[hegel::test(test_cases = 500)]
fn accumulate_render_round_trip(tc: TestCase) {
    let message = tc.draw(generators::assistant_message());
    let events = stream::draw_stream(&tc, &message);
    assert_eq!(accumulate(events), message);
}

/// While a block streams, the partial message shows a prefix of the
/// block's final text, and the prefix only grows.
#[hegel::test(test_cases = 500)]
fn partial_text_is_growing_prefix(tc: TestCase) {
    let message = tc.draw(generators::assistant_message());
    let events = stream::draw_stream(&tc, &message);
    let mut acc = Accumulator::new();
    let mut previous = vec![String::new(); message.content.len()];
    for event in events {
        acc.push(event).unwrap();
        let partial = acc.partial().unwrap();
        for (i, block) in partial.content.iter().enumerate() {
            let (now, last) = match (block, &message.content[i]) {
                (AssistantBlock::Text(p), AssistantBlock::Text(f)) => {
                    (&p.text, &f.text)
                }
                (AssistantBlock::Thinking(p), AssistantBlock::Thinking(f)) => {
                    (&p.thinking, &f.thinking)
                }
                (AssistantBlock::ToolCall(_), AssistantBlock::ToolCall(_)) => {
                    continue;
                }
                _ => panic!("block {i} changed kind"),
            };
            assert!(last.starts_with(now.as_str()), "{now:?} vs {last:?}");
            assert!(now.starts_with(previous[i].as_str()));
            previous[i] = now.clone();
        }
    }
}

/// The raw arguments of an open tool call are the concatenation of its
/// deltas so far.
#[hegel::test]
fn partial_arguments_concatenate_deltas(tc: TestCase) {
    let message = tc.draw(generators::assistant_message());
    let events = stream::draw_stream(&tc, &message);
    let mut acc = Accumulator::new();
    let mut expected = String::new();
    for event in events {
        match &event {
            AssistantEvent::ToolCallStart { .. } => expected.clear(),
            AssistantEvent::ToolCallDelta { delta, .. } => {
                expected.push_str(delta)
            }
            _ => {}
        }
        let in_call = matches!(
            event,
            AssistantEvent::ToolCallStart { .. }
                | AssistantEvent::ToolCallDelta { .. }
        );
        acc.push(event).unwrap();
        if in_call {
            assert_eq!(acc.partial_arguments(), Some(expected.as_str()));
        } else {
            assert_eq!(acc.partial_arguments(), None);
        }
    }
}

/// An error in the middle of a block ends the stream: the message keeps
/// the blocks so far, the cut-off block holds a prefix of its text, and
/// the stop reason and error text come from the error.
#[hegel::test]
fn error_mid_block_keeps_partial_content(tc: TestCase) {
    let mut message = tc.draw(generators::assistant_message());
    message.stop_reason = StopReason::Stop;
    let events = stream::draw_stream(&tc, &message);
    // Cut before the terminal event, at any point after Start.
    let cut = tc.draw(
        gs::integers::<usize>()
            .min_value(1)
            .max_value(events.len() - 1),
    );
    let error = tc.draw(generators::text(20));
    let mut acc = Accumulator::new();
    for event in events.into_iter().take(cut) {
        acc.push(event).unwrap();
    }
    acc.push(AssistantEvent::Error {
        reason: tau_ai::event::ErrorReason::Error,
        message: error.clone(),
        usage: Usage::default(),
        class: tau_ai::retry::Class::Fatal,
    })
    .unwrap();
    let result = acc.finish().unwrap();
    assert_eq!(result.stop_reason, StopReason::Error);
    assert_eq!(result.error_message, Some(error));
    assert!(result.content.len() <= message.content.len());
    for (got, full) in result.content.iter().zip(&message.content) {
        match (got, full) {
            (AssistantBlock::Text(g), AssistantBlock::Text(f)) => {
                assert!(f.text.starts_with(&g.text))
            }
            (AssistantBlock::Thinking(g), AssistantBlock::Thinking(f)) => {
                assert!(f.thinking.starts_with(&g.thinking))
            }
            (AssistantBlock::ToolCall(g), AssistantBlock::ToolCall(f)) => {
                assert_eq!((&g.id, &g.name), (&f.id, &f.name))
            }
            _ => panic!("block kind changed"),
        }
    }
}

/// Any event after the terminal event is rejected.
#[hegel::test]
fn events_after_terminal_are_rejected(tc: TestCase) {
    let message = tc.draw(generators::assistant_message());
    let events = stream::draw_stream(&tc, &message);
    let extra = tc.draw(gs::sampled_from(events.clone()));
    let mut acc = Accumulator::new();
    for event in events {
        acc.push(event).unwrap();
    }
    assert!(acc.is_finished());
    assert!(acc.push(extra).is_err());
}

fn start() -> AssistantEvent {
    AssistantEvent::Start {
        model: "gpt-5.5".into(),
        response_id: None,
        timestamp: 0,
    }
}

fn done() -> AssistantEvent {
    AssistantEvent::Done {
        reason: DoneReason::Stop,
        usage: Usage::default(),
        response_id: None,
    }
}

/// Opens block `index` of the given kind.
fn open(kind: usize, index: usize) -> AssistantEvent {
    match kind {
        0 => AssistantEvent::TextStart { index },
        1 => AssistantEvent::ThinkingStart { index },
        _ => AssistantEvent::ToolCallStart {
            index,
            id: "call_1|fc_1".into(),
            name: "search".into(),
        },
    }
}

/// Grammar violations, one per rule and block kind. Each case names the
/// position of the event that must be rejected.
#[test]
fn grammar_violations_are_rejected() {
    let delta = |index| AssistantEvent::TextDelta {
        index,
        delta: "x".into(),
    };
    let mut cases: Vec<(String, Vec<AssistantEvent>, usize)> = vec![
        ("delta before Start".into(), vec![delta(0)], 0),
        ("second Start".into(), vec![start(), start()], 1),
        (
            "delta for an unopened block".into(),
            vec![start(), delta(0)],
            1,
        ),
        (
            "delta for the wrong kind".into(),
            vec![
                start(),
                open(0, 0),
                AssistantEvent::ThinkingDelta {
                    index: 0,
                    delta: "x".into(),
                },
            ],
            2,
        ),
        (
            "Done with an open block".into(),
            vec![start(), open(0, 0), done()],
            2,
        ),
        (
            "event after Done".into(),
            vec![start(), done(), open(0, 0)],
            2,
        ),
    ];
    for kind in 0..3 {
        cases.push((
            format!("kind {kind}: index skips ahead"),
            vec![start(), open(kind, 1)],
            1,
        ));
        for other in 0..3 {
            cases.push((
                format!("kind {kind} opened inside kind {other}"),
                vec![start(), open(other, 0), open(kind, 1)],
                2,
            ));
        }
    }
    for (name, events, position) in cases {
        let mut acc = Accumulator::new();
        let error = events
            .into_iter()
            .find_map(|e| acc.push(e).err())
            .unwrap_or_else(|| panic!("{name}: accepted"));
        assert_eq!(error.position, position, "{name}");
        let shown = error.to_string();
        assert!(
            shown.starts_with(&format!("event {position}: "))
                && shown.contains(error.reason),
            "{name}: {shown}"
        );
    }
}

/// `is_finished` turns true exactly at the terminal event.
#[hegel::test]
fn finished_exactly_at_terminal(tc: TestCase) {
    let message = tc.draw(generators::assistant_message());
    let events = stream::draw_stream(&tc, &message);
    let last = events.len() - 1;
    let mut acc = Accumulator::new();
    for (i, event) in events.into_iter().enumerate() {
        assert!(!acc.is_finished());
        acc.push(event).unwrap();
        assert_eq!(acc.is_finished(), i == last);
    }
}

/// A stream without a terminal event has no final message.
#[test]
fn unterminated_stream_has_no_message() {
    let mut acc = Accumulator::new();
    acc.push(start()).unwrap();
    assert!(acc.finish().is_err());
    assert!(Accumulator::new().finish().is_err());
}
