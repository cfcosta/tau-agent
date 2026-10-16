//! The agents' shared types: the event message an agent reads and its
//! card reads back, and names. What an agent starts with is checked
//! where it shows: its first turns and its cap, in `tau-agents-host`.

use hegel::generators::{self as gs, Generator as _};
use serde_json::{Value, json};
use tau_agents::{
    event::{Batch, Event, INLINE_DATA, Kind, Message, clock},
    valid_name,
};

/// A name or an id: any short text, mostly of letters, but with line
/// breaks and the line's separator `·` anywhere, beside each other, at
/// either end or alone.
fn field_text(tc: &hegel::TestCase, alphabet: &str) -> String {
    tc.draw(hegel::one_of!(
        gs::text().alphabet("ab:· \n\r").max_size(10),
        gs::text().alphabet(alphabet).min_size(1).max_size(10),
    ))
}

fn event(tc: &hegel::TestCase) -> Event {
    Event {
        trigger: field_text(tc, "abc_"),
        kind: tc.draw(gs::sampled_from(Kind::ALL.to_vec()).print_as_debug()),
        id: field_text(tc, "abc:019"),
        at: tc
            .draw(gs::integers::<i64>().min_value(0).max_value(4_000_000_000)),
        data: match tc.draw(gs::integers::<u8>().max_value(2)) {
            0 => Value::Null,
            1 => json!({ "text": tc.draw(gs::text().max_size(40)) }),
            _ => json!([tc.draw(gs::integers::<i64>())]),
        },
    }
}

/// What an event message says of each event reads back from it, at any
/// offset from UTC: the card shows what the agent read. A name or an id
/// stays on its line and never holds the line's separator, whatever it
/// held: line breaks and separators read back as spaces, and nothing
/// else of it is lost. One that held neither reads back as it was.
#[hegel::test(test_cases = 300)]
fn an_event_message_reads_back_as_its_events(tc: hegel::TestCase) {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(5));
    let events: Vec<Event> = (0..count).map(|_| event(&tc)).collect();
    let offset = tc.draw(
        gs::integers::<i32>()
            .min_value(-14 * 3600)
            .max_value(14 * 3600),
    );
    let batch = Batch {
        events: events.clone(),
    };
    let message = Message::parse(&batch.message(offset))
        .expect("an event message reads back");
    assert_eq!(message.events.len(), events.len());
    // What is left of a field once spaces and separators are taken out.
    let words = |field: &str| -> String {
        field
            .chars()
            .filter(|c| !matches!(c, ' ' | '\n' | '\r' | '·'))
            .collect()
    };
    for (shown, event) in message.events.iter().zip(&events) {
        for (shown, written) in
            [(&shown.trigger, &event.trigger), (&shown.id, &event.id)]
        {
            if written.contains(['\n', '\r', '·']) {
                tc.event("a field kept on its line");
            } else {
                assert_eq!(shown, written);
            }
            assert!(!shown.contains(['\n', '\r']), "{shown:?}");
            assert!(!format!(" {shown} ").contains(" · "), "{shown:?}");
            assert_eq!(words(shown), words(written), "{written:?}");
        }
        assert_eq!(shown.kind, event.kind);
        assert_eq!(shown.time, clock(event.at, offset));
        let data: Value = serde_json::from_str(&shown.data).unwrap();
        assert_eq!(data, event.data);
    }
}

#[test]
fn an_event_message_reads_as_the_decision_shows() {
    let batch = Batch {
        events: vec![
            Event {
                trigger: "evening".into(),
                kind: Kind::Schedule,
                id: "evening:64800".into(),
                at: 64_800,
                data: json!({ "due": 64_800 }),
            },
            Event {
                trigger: "triage".into(),
                kind: Kind::Command,
                id: "cmd:1".into(),
                at: 64_830,
                data: json!({ "args": "412" }),
            },
        ],
    };
    assert_eq!(
        batch.message(3600),
        "[events · 2 · 19:00:00–19:00:30]\n\
         - evening · schedule · evening:64800 · 19:00:00\n  {\"due\":64800}\n\
         - triage · command · cmd:1 · 19:00:30\n  {\"args\":\"412\"}"
    );
    let one = Batch {
        events: batch.events[..1].to_vec(),
    };
    assert!(one.message(0).starts_with("[events · 1 · 18:00:00]\n"));
}

/// Data whose JSON fits [`INLINE_DATA`] bytes is held whole; longer data
/// is cut at the last character that ends within the limit, never inside
/// one, followed by how many bytes it had in all. Lengths gather at the
/// limit, and characters of one to four bytes reach the cut.
#[hegel::test(test_cases = 300)]
fn data_past_the_inline_limit_is_cut_at_a_character_and_says_its_size(
    tc: hegel::TestCase,
) {
    let json_bytes = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![
            INLINE_DATA - 1,
            INLINE_DATA,
            INLINE_DATA + 1,
            INLINE_DATA + 2,
            INLINE_DATA + 3,
        ]),
        gs::integers::<usize>()
            .min_value(2)
            .max_value(INLINE_DATA + 40),
    ));
    let pattern: Vec<char> = tc.draw(
        gs::vecs(gs::sampled_from(vec!['a', 'é', '日', '🦀']))
            .min_size(1)
            .max_size(6),
    );
    // A string whose JSON, its quotes included, is `json_bytes` long:
    // the pattern while it fits, then single bytes.
    let mut text = String::new();
    for c in pattern.iter().cycle() {
        if text.len() + c.len_utf8() > json_bytes - 2 {
            break;
        }
        text.push(*c);
    }
    while text.len() < json_bytes - 2 {
        text.push('a');
    }
    let json = format!("\"{text}\"");
    assert_eq!(json.len(), json_bytes);
    let batch = Batch {
        events: vec![Event {
            trigger: "feed".into(),
            kind: Kind::Poll,
            id: "feed:1".into(),
            at: 0,
            data: json!(text),
        }],
    };
    let message = Message::parse(&batch.message(0)).unwrap();
    let expected = if json_bytes <= INLINE_DATA {
        tc.event("whole");
        json
    } else {
        let mut head = String::new();
        for c in json.chars() {
            if head.len() + c.len_utf8() > INLINE_DATA {
                break;
            }
            head.push(c);
        }
        if head.len() < INLINE_DATA {
            tc.event("cut before a character that crosses the limit");
        }
        format!("{head} … (cut: {json_bytes} bytes in all)")
    };
    assert_eq!(message.events[0].data, expected);
}

/// Text that is not an event message is not read as one.
#[test]
fn other_text_is_not_an_event_message() {
    for text in [
        "",
        "hello",
        "[events · 2 · 10:00:00]\n- a · poll · a:1 · 10:00:00\n  null",
        "[events · 1 · 10:00:00]\n- a · nope · a:1 · 10:00:00\n  null",
        "[events · 1 · 10:00:00]\n- a · poll · a:1 · 10:00:00",
    ] {
        assert_eq!(Message::parse(text), None, "{text:?}");
    }
}

/// What an agent's name must be (ADR 0037): one to forty characters of
/// lower-case ASCII letters, digits and dashes, starting with a letter.
/// The oracle is that rule as a regular expression. Lengths reach both
/// ends of the range and one past each; names are drawn in the rule's
/// alphabet, valid but for their length, or from a wider one with
/// capitals, underscores, spaces, slashes and `é`.
#[hegel::test(test_cases = 500)]
fn a_name_is_lower_case_words_with_dashes(tc: hegel::TestCase) {
    let rule = regex::Regex::new(r"\A[a-z][a-z0-9-]{0,39}\z").unwrap();
    let length = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![0usize, 1, 2, 39, 40, 41]),
        gs::integers::<usize>().max_value(42),
    ));
    let name: String = if length > 0 && tc.draw(gs::booleans()) {
        let first: String =
            tc.draw(gs::text().alphabet("abcxyz").min_size(1).max_size(1));
        let rest: String = tc.draw(
            gs::text()
                .alphabet("abcxyz019-")
                .min_size(length - 1)
                .max_size(length - 1),
        );
        first + &rest
    } else {
        tc.draw(
            gs::text()
                .alphabet("abcxyzAZ019-_ /é")
                .min_size(length)
                .max_size(length),
        )
    };
    let valid = rule.is_match(&name);
    tc.event(if valid { "valid" } else { "invalid" });
    assert_eq!(valid_name(&name), valid, "{name:?}");
}
