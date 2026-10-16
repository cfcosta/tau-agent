//! What holds for any questions, any keys and any records: questions
//! within the limits are asked and any one fault is refused; however the
//! person works the panel, what it sends answers what was asked; the
//! model reads every answer and note; the fold shows the call that
//! waits.

use hegel::generators::{self as gs, Generator as _};
use tau_ui_plugin::Fold as _;

hegel::pretty_print_as_debug!(Step);
use tau_ask::{
    Answer,
    Ask,
    Choice,
    Question,
    Record,
    Reply,
    ask::{MAX_CHOICES, MAX_HEADER, MAX_QUESTIONS, OTHER},
    ui::{Draft, Key, State, Then},
};

/// Distinct words, so labels and questions never repeat by chance.
fn word(tc: &hegel::TestCase, n: usize) -> String {
    let stem = tc.draw(gs::from_regex(r"[a-z]{1,6}"));
    format!("{stem}{n}")
}

/// `count` characters a header may hold, of one to three bytes each,
/// none of them white space at an end.
fn header_of(tc: &hegel::TestCase, count: usize) -> String {
    let inner = tc.draw(
        gs::text()
            .alphabet("aZ1 é漢")
            .min_size(count.saturating_sub(2))
            .max_size(count.saturating_sub(2)),
    );
    let end = || gs::sampled_from(vec!['a', 'é', '漢']);
    let first = tc.draw(end());
    if count == 1 {
        return first.to_string();
    }
    format!("{first}{inner}{}", tc.draw(end()))
}

fn question(tc: &hegel::TestCase, n: usize) -> Question {
    let multi = tc.draw(gs::booleans());
    let choices =
        tc.draw(gs::integers::<usize>().min_value(2).max_value(MAX_CHOICES));
    // The shortest and longest headers as often as the rest.
    let header = tc.draw(hegel::one_of!(
        gs::sampled_from(vec![1, MAX_HEADER]),
        gs::integers::<usize>().min_value(1).max_value(MAX_HEADER),
    ));
    Question {
        question: format!("{}?", word(tc, n)),
        header: header_of(tc, header),
        options: (0..choices)
            .map(|k| Choice {
                label: word(tc, k),
                description: word(tc, k),
                preview: (!multi && tc.draw(gs::booleans()))
                    .then(|| word(tc, k)),
            })
            .collect(),
        multi_select: multi,
    }
}

#[hegel::composite]
fn ask(tc: &hegel::TestCase) -> Ask {
    let n = tc.draw(
        gs::integers::<usize>()
            .min_value(1)
            .max_value(MAX_QUESTIONS),
    );
    Ask {
        questions: (0..n).map(|i| question(tc, i)).collect(),
    }
}

/// Questions within the limits are asked, a header of 1 to
/// [`MAX_HEADER`] characters however many bytes they take; each kind of
/// fault alone is refused, with a reason that names the question at
/// fault. Text is compared trimmed: a question or a label repeated with
/// white space around it is still repeated.
#[hegel::test(test_cases = 300)]
fn questions_within_the_limits_are_asked_and_faults_refused(
    tc: hegel::TestCase,
) {
    let ask = tc.draw(ask().print_as_debug());
    assert_eq!(ask.check(), Ok(()));
    let mut wrong = ask.clone();
    let i = tc.draw(
        gs::integers::<usize>()
            .min_value(0)
            .max_value(ask.questions.len() - 1),
    );
    // The question at fault, when the fault is in one.
    let mut at = Some(i);
    match tc.draw(gs::integers::<u8>().min_value(0).max_value(7)) {
        0 => {
            wrong.questions.clear();
            at = None;
        }
        1 => {
            let extra = wrong.questions[0].clone();
            while wrong.questions.len() <= MAX_QUESTIONS {
                let mut more = extra.clone();
                more.question =
                    format!("{} {}", more.question, wrong.questions.len());
                wrong.questions.push(more);
            }
            at = None;
        }
        2 => {
            let count = tc.draw(hegel::one_of!(
                gs::just(MAX_HEADER + 1),
                gs::integers::<usize>()
                    .min_value(MAX_HEADER + 1)
                    .max_value(MAX_HEADER + 6),
            ));
            wrong.questions[i].header = header_of(&tc, count);
        }
        3 => wrong.questions[i].options.truncate(1),
        4 => wrong.questions[i].options[1].label = OTHER.to_lowercase(),
        5 => {
            wrong.questions[i].options[1].label =
                format!(" {} ", wrong.questions[i].options[0].label)
        }
        6 => {
            wrong.questions[i].multi_select = true;
            wrong.questions[i].options[0].preview = Some("x".into());
        }
        _ => {
            let again = format!(" {}\t", wrong.questions[0].question);
            if wrong.questions.len() < 2 {
                let mut copy = wrong.questions[0].clone();
                copy.question = again;
                wrong.questions.push(copy);
            } else {
                wrong.questions[1].question = again;
            }
            at = Some(1);
        }
    }
    let why = wrong.check().expect_err("one fault is refused");
    if let Some(at) = at {
        let named = format!("Question {}", at + 1);
        assert!(why.starts_with(&named), "{why}");
    }
}

/// What the person does in the panel.
#[derive(Debug, Clone)]
enum Step {
    Key(Key),
    Click(usize),
    Other(String),
    Note(String),
    Go(usize),
}

#[hegel::composite]
fn step(tc: &hegel::TestCase) -> Step {
    match tc.draw(gs::integers::<u8>().min_value(0).max_value(12)) {
        0 => Step::Key(Key::Up),
        1 => Step::Key(Key::Down),
        2 => Step::Key(Key::Left),
        3 => Step::Key(Key::Right),
        4 => Step::Key(Key::Enter),
        5 => Step::Key(Key::Space),
        6 => Step::Key(Key::Note),
        7 => Step::Key(Key::Digit(
            tc.draw(gs::integers::<usize>().min_value(1).max_value(9)),
        )),
        8 => Step::Click(
            tc.draw(gs::integers::<usize>().min_value(0).max_value(6)),
        ),
        9 => Step::Other(
            tc.draw(gs::sampled_from(vec!["", "  ", "my own", "two\nlines"]))
                .into(),
        ),
        10 => Step::Note(
            tc.draw(gs::sampled_from(vec!["", "a note", " spaced "]))
                .into(),
        ),
        _ => {
            Step::Go(tc.draw(gs::integers::<usize>().min_value(0).max_value(6)))
        }
    }
}

/// However the person works the panel, it stays on a question or the
/// review, a one-answer question holds at most one answer, and what it
/// would send answers what was asked; Send comes only when it is ready.
#[hegel::test(test_cases = 300)]
fn whatever_the_person_does_the_panel_sends_a_fitting_reply(
    tc: hegel::TestCase,
) {
    let ask = tc.draw(ask().print_as_debug());
    let steps: Vec<Step> = tc.draw(gs::vecs(step()).max_size(40));
    let mut draft = Draft::new("call_1", &ask);
    for step in steps {
        let then = match step {
            Step::Key(key) => draft.key(&ask, key),
            Step::Click(row) => draft.choose(&ask, row, false),
            Step::Other(text) => {
                draft.write_other(&ask, &text);
                Then::Stay
            }
            Step::Note(text) => {
                draft.write_note(&ask, &text);
                Then::Stay
            }
            Step::Go(tab) => {
                draft.go(&ask, tab);
                Then::Stay
            }
        };
        assert!(draft.tab <= ask.questions.len());
        for (i, question) in ask.questions.iter().enumerate() {
            assert!(draft.cursor[i] <= question.options.len());
            let answer = draft.answer(&ask, i);
            let given =
                answer.picked.len() + usize::from(answer.other.is_some());
            if !question.multi_select {
                assert!(given <= 1, "{answer:?}");
            }
            assert_eq!(draft.answered(i), given > 0);
        }
        if then == Then::Send {
            assert!(draft.reviewing(&ask) && draft.ready());
        }
        match draft.reply(&ask) {
            Some(reply) => {
                assert!(draft.ready());
                assert_eq!(reply.check(&ask), Ok(()));
            }
            None => assert!(!draft.ready()),
        }
    }
}

/// The model reads each question with its answer (choices and the
/// person's own, trimmed), and each note.
#[hegel::test(test_cases = 200)]
fn the_model_reads_every_answer_and_note(tc: hegel::TestCase) {
    let ask = tc.draw(ask().print_as_debug());
    let answers: Vec<Answer> = ask
        .questions
        .iter()
        .map(|question| {
            // A checklist picks any of its choices, in order; a question
            // where one is picked takes a choice or the person's own.
            let mut picked: Vec<String> = question
                .options
                .iter()
                .filter(|_| tc.draw(gs::booleans()))
                .map(|choice| choice.label.clone())
                .collect();
            if !question.multi_select {
                picked.truncate(1);
            }
            let own = picked.is_empty() || tc.draw(gs::booleans());
            if own && !question.multi_select {
                picked.clear();
            }
            Answer {
                picked,
                other: own.then(|| {
                    tc.draw(gs::sampled_from(vec!["my own", "  padded  "]))
                        .to_owned()
                }),
                note: tc
                    .draw(gs::optional(gs::sampled_from(vec![
                        "first line\nsecond",
                        " short ",
                    ])))
                    .map(String::from),
            }
        })
        .collect();
    let reply = Reply::Answered {
        answers: answers.clone(),
    };
    assert_eq!(reply.check(&ask), Ok(()));
    // As ask.md writes it: each question with its picks in the
    // question's order, then the person's own answer, trimmed, joined
    // by commas; a note under it, its lines kept in line.
    let mut want = String::from("The person answered:");
    for (question, answer) in ask.questions.iter().zip(&answers) {
        let own = answer.other.as_deref().map(str::trim);
        let parts: Vec<&str> = answer
            .picked
            .iter()
            .map(String::as_str)
            .chain(own.filter(|own| !own.is_empty()))
            .collect();
        want.push_str(&format!(
            "\n\"{}\" = \"{}\"",
            question.question,
            parts.join(", ")
        ));
        if let Some(note) = answer.note.as_deref().map(str::trim) {
            want.push_str("\n  note: ");
            want.push_str(&note.lines().collect::<Vec<_>>().join("\n        "));
        }
    }
    assert_eq!(reply.text(&ask), want);
    assert!(
        Reply::Declined
            .text(&ask)
            .starts_with("The person declined to answer.")
    );
}

#[hegel::composite]
fn record(tc: &hegel::TestCase) -> Record {
    let call = format!(
        "call_{}",
        tc.draw(gs::integers::<u8>().min_value(1).max_value(3))
    );
    match tc.draw(gs::integers::<u8>().min_value(0).max_value(2)) {
        0 => Record::Asked {
            call,
            ask: tc.draw_silent(ask()),
        },
        1 => Record::Answered {
            call,
            reply: Reply::Declined,
        },
        _ => Record::Closed { call },
    }
}

/// Over any records, the call the panel shows is the first, in the order
/// calls first asked, whose latest record is its asking; and records read
/// back as written.
#[hegel::test(test_cases = 300)]
fn the_fold_shows_the_first_call_still_waiting(tc: hegel::TestCase) {
    let records: Vec<Record> =
        tc.draw(gs::vecs(record().print_as_debug()).max_size(12));
    let mut state = State::default();
    for record in &records {
        assert_eq!(Record::parse(&record.to_value()).as_ref(), Some(record));
        state.apply(
            record.clone(),
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
    }
    let mut order: Vec<&str> = Vec::new();
    for record in &records {
        if matches!(record, Record::Asked { .. })
            && !order.contains(&record.call())
        {
            order.push(record.call());
        }
    }
    let expected = order.into_iter().find(|call| {
        let last = records.iter().rev().find(|r| r.call() == *call);
        matches!(last, Some(Record::Asked { .. }))
    });
    assert_eq!(state.waiting().map(|c| c.call.as_str()), expected);
    // A body that is not a record reads as none, so nothing folds it.
    assert!(
        Record::parse(&serde_json::json!({ "kind": "nonsense" })).is_none()
    );
}
