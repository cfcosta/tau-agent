//! The fold: records into notes, anchors and the band.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use serde_json::json;
use tau_ui_plugin::testing::{FakeRun, fold};
use tau_watcher::{
    WatcherUi,
    cadence::{BAND_LIMIT, SEEN},
    record::{Answer, NAME, Record, Tag},
    state::{Note, State, Status},
};

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

/// A history's records: sometimes a run of notes past the seen-list's
/// cap first, so the cap is reached, then notes, drops, messages
/// written past and answers in any order. An answer names one of the
/// first notes, one past the cap, or one that is not there.
#[hegel::composite]
fn history(tc: &TestCase) -> Vec<Record> {
    let leading = tc.draw(gs::sampled_from(vec![
        0_usize,
        0,
        0,
        SEEN - 1,
        SEEN,
        SEEN + 1,
        SEEN + 6,
    ]));
    let mut records: Vec<Record> = (0..leading)
        .map(|n| Record::Noted {
            step: n as u32,
            tag: Tag::HeadsUp,
            line: format!("Lead {n}."),
            explain: None,
        })
        .collect();
    let more = tc.draw(gs::integers::<usize>().max_value(60));
    for _ in 0..more {
        let record = match tc.draw(gs::integers::<u8>().max_value(3)) {
            0 => Record::Noted {
                step: tc.draw(gs::integers::<u32>().max_value(500)),
                tag: tc
                    .draw(gs::sampled_from(Tag::ALL.to_vec()).print_as_debug()),
                line: tc.draw(gs::from_regex("[a-z]{1,8}\\.").fullmatch(true)),
                explain: None,
            },
            1 => Record::Dropped {
                step: tc.draw(gs::integers::<u32>().max_value(500)),
                reason: "x".into(),
            },
            2 => Record::TypedPast,
            _ => Record::Answered {
                key: tc
                    .draw(gs::sampled_from(vec![
                        "n0", "n1", "n2", "n9", "n50", "n56", "n99",
                    ]))
                    .to_owned(),
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
        };
        records.push(record);
    }
    records
}

/// The fold as the plugin's docs state it, over plain fields.
#[derive(Default)]
struct Model {
    /// Each note's key, step, tag, line, status and times written past.
    notes: Vec<(String, u32, Tag, String, Status, u32)>,
    ignored: u32,
    dropped: u32,
    last_check: Option<u32>,
}

impl Model {
    fn apply(&mut self, record: &Record) {
        match record {
            // A note takes the next anchor, `n0`, `n1`, ..., and waits.
            Record::Noted {
                step, tag, line, ..
            } => {
                let key = format!("n{}", self.notes.len());
                self.notes.push((
                    key,
                    *step,
                    *tag,
                    line.clone(),
                    Status::New,
                    0,
                ));
                self.last_check = Some(*step);
            }
            Record::Dropped { step, .. } => {
                self.dropped += 1;
                self.last_check = Some(*step);
            }
            // A message written while the newest note waits unanswered
            // is written past it, and counts toward the back-off.
            Record::TypedPast => {
                if let Some(note) = self.notes.last_mut()
                    && note.4 == Status::New
                {
                    note.5 += 1;
                    self.ignored += 1;
                }
            }
            // Answering a note gives it that status and ends the
            // back-off; an answer for no note changes nothing.
            Record::Answered { key, answer } => {
                if let Some(note) =
                    self.notes.iter_mut().find(|note| &note.0 == key)
                {
                    note.4 = match answer {
                        Answer::Learned => Status::Learned,
                        Answer::Knew => Status::Knew,
                        Answer::Chatted => Status::Chatted,
                        Answer::Dismissed => Status::Dismissed,
                    };
                    self.ignored = 0;
                }
            }
        }
    }

    /// The last [`SEEN`] of `lines`.
    fn last(lines: Vec<String>) -> Vec<String> {
        let skip = lines.len().saturating_sub(SEEN);
        lines.into_iter().skip(skip).collect()
    }
}

/// Whatever happens, in whatever order, the fold is the model's after
/// every record: the notes with their anchors, statuses and times
/// written past, the back-off count, the drops and the last check; the
/// band is the newest note while it is new and written past fewer than
/// [`BAND_LIMIT`] times; a message is written past a note while the
/// newest one is new; the seen-list is the last [`SEEN`] lines and the
/// known-list the last [`SEEN`] the person knew. Folding the stored
/// records again gives the same state.
#[hegel::test(test_cases = 200)]
fn any_history_folds_as_the_model_does(tc: TestCase) {
    let records = tc.draw(history().print_as_debug());
    let mut state = State::default();
    let mut run = FakeRun::default();
    let mut model = Model::default();
    let mut bodies = Vec::new();
    for record in &records {
        let body = serde_json::to_value(record).unwrap();
        fold(WatcherUi, &mut state, &body, &mut run);
        model.apply(record);
        bodies.push(body);

        let notes: Vec<(String, u32, Tag, String, Status, u32)> = state
            .notes
            .iter()
            .map(|note: &Note| {
                (
                    note.key.clone(),
                    note.step,
                    note.tag,
                    note.line.clone(),
                    note.status,
                    note.typed_past,
                )
            })
            .collect();
        assert_eq!(notes, model.notes, "{record:?}");
        let keys: Vec<&str> =
            model.notes.iter().map(|note| note.0.as_str()).collect();
        assert_eq!(run.anchors, keys);
        assert_eq!(
            (state.ignored, state.dropped, state.last_check),
            (model.ignored, model.dropped, model.last_check)
        );
        let newest = model.notes.last();
        let band = newest
            .filter(|note| note.4 == Status::New && note.5 < BAND_LIMIT)
            .map(|note| note.0.as_str());
        assert_eq!(state.band().map(|note| note.key.as_str()), band);
        assert_eq!(state.waiting(), band.is_some());
        assert_eq!(
            state.unanswered(),
            newest.is_some_and(|note| note.4 == Status::New)
        );
        let lines = |knew: bool| {
            Model::last(
                model
                    .notes
                    .iter()
                    .filter(|note| !knew || note.4 == Status::Knew)
                    .map(|note| note.3.clone())
                    .collect(),
            )
        };
        assert_eq!(state.seen(), lines(false));
        assert_eq!(state.known(), lines(true));
    }
    assert_eq!(State::from_records(&bodies), state);
}
