//! The fold: records into notes, anchors and the band.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use serde_json::json;
use tau_ui_plugin::testing::{FakeRun, fold};
use tau_watcher::{
    WatcherUi,
    cadence::{BAND_LIMIT, SEEN},
    record::{Answer, NAME, Record, Tag},
    state::{State, Status},
};

fn noted(step: u32, line: &str) -> Record {
    Record::Noted {
        step,
        tag: Tag::HeadsUp,
        line: line.into(),
        explain: None,
    }
}

fn answered(key: &str, answer: Answer) -> Record {
    Record::Answered {
        key: key.into(),
        answer,
    }
}

fn state_of(records: &[Record]) -> State {
    let mut state = State::default();
    for record in records {
        state.record(record.clone());
    }
    state
}

#[test]
fn a_note_is_anchored_in_the_transcript_and_waits() {
    let mut state = State::default();
    let mut run = FakeRun::default();
    fold(
        WatcherUi,
        &mut state,
        &serde_json::to_value(noted(6, "The key expires.")).unwrap(),
        &mut run,
    );
    assert_eq!(run.anchors, ["n0"]);
    assert_eq!(state.notes[0].status, Status::New);
    assert!(state.waiting());
    assert_eq!(state.band().map(|note| note.key.as_str()), Some("n0"));
}

#[test]
fn a_dropped_reply_shows_nothing() {
    let mut state = State::default();
    let mut run = FakeRun::default();
    let body = json!({"kind": "dropped", "step": 6, "reason": "empty"});
    fold(WatcherUi, &mut state, &body, &mut run);
    assert!(run.anchors.is_empty());
    assert!(state.notes.is_empty());
    assert_eq!(state.dropped, 1);
    assert_eq!(state.last_check, Some(6));
}

#[test]
fn acting_on_a_note_takes_the_band_away_and_keeps_the_note() {
    for answer in [
        Answer::Learned,
        Answer::Knew,
        Answer::Chatted,
        Answer::Dismissed,
    ] {
        let state = state_of(&[noted(6, "A."), answered("n0", answer)]);
        assert!(state.band().is_none(), "{answer:?}");
        assert!(!state.waiting());
        assert_eq!(state.notes.len(), 1);
    }
}

#[test]
fn a_note_leaves_the_band_when_written_past_twice() {
    let mut state = state_of(&[noted(6, "A.")]);
    for written in 1..=BAND_LIMIT {
        assert!(state.band().is_some());
        state.record(Record::TypedPast);
        assert_eq!(state.ignored, written);
    }
    assert!(state.band().is_none());
    // Not answered: the next message is still past it, and the checks
    // are no longer held back.
    assert!(state.unanswered());
    assert!(!state.waiting());
}

#[test]
fn answering_ends_the_back_off() {
    let mut state = state_of(&[noted(6, "A.")]);
    for _ in 0..4 {
        state.record(Record::TypedPast);
    }
    assert_eq!(state.ignored, 4);
    state.record(answered("n0", Answer::Dismissed));
    assert_eq!(state.ignored, 0);
}

#[test]
fn typing_with_no_note_waiting_counts_nothing() {
    let mut state = state_of(&[noted(6, "A."), answered("n0", Answer::Knew)]);
    state.record(Record::TypedPast);
    assert_eq!(state.ignored, 0);
    assert!(!state.unanswered());
}

#[test]
fn only_the_lines_the_person_knew_are_known() {
    let state = state_of(&[
        noted(6, "One."),
        answered("n0", Answer::Knew),
        noted(12, "Two."),
        answered("n1", Answer::Dismissed),
    ]);
    assert_eq!(state.known(), ["One."]);
    assert_eq!(state.seen(), ["One.", "Two."]);
}

#[test]
fn records_that_do_not_read_are_skipped_in_stored_state() {
    let bodies = vec![
        json!({"kind": "noted", "step": 6, "tag": "heads_up", "line": "A."}),
        json!({"kind": "something_new"}),
        json!({"kind": "answered", "key": "n0", "answer": "knew"}),
    ];
    let state = State::from_records(&bodies);
    assert_eq!(state.notes.len(), 1);
    assert_eq!(state.notes[0].status, Status::Knew);
    let _ = NAME;
}

#[hegel::composite]
fn record(tc: &TestCase) -> Record {
    let key =
        || gs::sampled_from(vec!["n0", "n1", "n2", "n9"]).map(str::to_owned);
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => Record::Noted {
            step: tc.draw(gs::integers::<u32>().max_value(500)),
            tag: Tag::HeadsUp,
            line: tc.draw(gs::from_regex("[a-z]{1,8}\\.")),
            explain: None,
        },
        1 => Record::Dropped {
            step: tc.draw(gs::integers::<u32>().max_value(500)),
            reason: "x".into(),
        },
        2 => Record::TypedPast,
        _ => Record::Answered {
            key: tc.draw(key().print_as_debug()),
            answer: tc.draw(
                gs::sampled_from(vec![
                    Answer::Learned,
                    Answer::Knew,
                    Answer::Chatted,
                    Answer::Dismissed,
                ])
                .print_as_debug(),
            ),
        },
    }
}

/// Whatever happens, in whatever order: one anchor per note, in order;
/// a band only for the newest note, when new; the seen-list within its
/// cap; the back-off count never above what was typed.
#[hegel::test(test_cases = 200)]
fn any_history_folds_consistently(tc: TestCase) {
    let records: Vec<Record> =
        tc.draw(gs::vecs(record().print_as_debug()).max_size(150));
    let mut state = State::default();
    let mut run = FakeRun::default();
    let mut typed = 0;
    for record in records {
        if record == Record::TypedPast {
            typed += 1;
        }
        let body = serde_json::to_value(&record).unwrap();
        fold(WatcherUi, &mut state, &body, &mut run);
    }
    let keys: Vec<&str> = state.notes.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(run.anchors, keys);
    if let Some(band) = state.band() {
        assert_eq!(band.key, state.notes.last().unwrap().key);
        assert_eq!(band.status, Status::New);
        assert!(band.typed_past < BAND_LIMIT);
    }
    assert!(state.seen().len() <= SEEN);
    assert!(state.known().len() <= SEEN);
    assert!(state.ignored <= typed);
    // Folding the stored records again gives the same state.
}
