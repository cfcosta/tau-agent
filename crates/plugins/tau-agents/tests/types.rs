//! The agents' shared types: the event message an agent reads and its
//! card reads back, names, and what an agent starts with.

use hegel::generators::{self as gs, Generator as _};
use serde_json::{Value, json};
use tau_agents::{
    Settings,
    event::{Batch, Event, INLINE_DATA, Kind, Message, clock},
    valid_name,
};

fn event(tc: &hegel::TestCase) -> Event {
    Event {
        trigger: tc.draw(gs::from_regex("[a-z_]{1,12}")),
        kind: tc.draw(gs::sampled_from(Kind::ALL.to_vec()).print_as_debug()),
        id: tc.draw(gs::from_regex("[a-z]{1,8}:[0-9a-z]{1,10}")),
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
/// offset from UTC: the card shows what the agent read.
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
    for (shown, event) in message.events.iter().zip(&events) {
        assert_eq!(shown.trigger, event.trigger);
        assert_eq!(shown.kind, event.kind);
        assert_eq!(shown.id, event.id);
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

/// A name or an id that would break a line is kept on it.
#[test]
fn a_field_that_would_break_its_line_is_kept_on_it() {
    let batch = Batch {
        events: vec![Event {
            trigger: "odd · name\nhere".into(),
            kind: Kind::Message,
            id: "a\nb".into(),
            at: 0,
            data: json!("hi"),
        }],
    };
    let message = Message::parse(&batch.message(0)).unwrap();
    assert_eq!(message.events[0].trigger, "odd name here");
    assert_eq!(message.events[0].id, "a b");
}

/// Data larger than a message holds is cut, and says so.
#[test]
fn large_data_is_cut_and_says_how_large_it_was() {
    let big = "é".repeat(INLINE_DATA);
    let batch = Batch {
        events: vec![Event {
            trigger: "feed".into(),
            kind: Kind::Poll,
            id: "feed:1".into(),
            at: 0,
            data: json!(big),
        }],
    };
    let message = Message::parse(&batch.message(0)).unwrap();
    let data = &message.events[0].data;
    assert!(data.len() < INLINE_DATA + 64, "{}", data.len());
    assert!(data.ends_with(&format!("(cut: {} bytes in all)", big.len() + 2)));
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

#[test]
fn names_are_lower_case_words_with_dashes() {
    for name in ["triage", "pr-triage", "ci2", "a"] {
        assert!(valid_name(name), "{name}");
    }
    let long = "a".repeat(41);
    for name in ["", "Triage", "2fast", "-x", "a b", "a/b", "a_b", "é", &long]
    {
        assert!(!valid_name(name), "{name}");
    }
}

/// A new agent runs on the plan's newest small model, at low effort, may
/// spend a dollar a day and gathers events for half a minute.
#[test]
fn an_agent_starts_small_and_cheap() {
    let settings: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(settings, Settings::default());
    assert_eq!(settings.daily_cap, 1.0);
    assert_eq!(settings.window_secs, 30);
    assert_eq!(settings.effort(), "low");
    assert!(settings.model().contains("luna"), "{}", settings.model());
}

/// What the person allows on top of the cap is for the day they allowed
/// it.
#[test]
fn more_allowed_is_for_its_day_only() {
    let settings = Settings {
        extra_today: 2.5,
        extra_day: Some("2026-10-11".into()),
        ..Settings::default()
    };
    assert_eq!(settings.cap_on("2026-10-11"), 3.5);
    assert_eq!(settings.cap_on("2026-10-12"), 1.0);
}
