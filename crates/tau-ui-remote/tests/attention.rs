//! What a chat's row says about whether it needs the person
//! (`tau_ui_remote::attention`): one state from any facts about a run,
//! the same as a small reference that lists every state the facts allow
//! and takes the most pressing.

use hegel::{
    TestCase,
    generators::{self as gs, Generator as _},
};
use tau_ui_remote::attention::{Attention, Facts, Forecast, Status};

#[hegel::composite]
fn facts(tc: &TestCase) -> Facts {
    let status = tc.draw(
        gs::sampled_from(vec![
            Status::Live,
            Status::Stopped,
            Status::Failed,
            Status::Interrupted,
        ])
        .print_as_debug(),
    );
    let forecast = tc.draw(gs::booleans()).then(|| {
        let files = tc.draw(gs::integers::<usize>().max_value(3));
        Forecast {
            changes: tc.draw(gs::integers::<usize>().max_value(4)),
            conflicts: (0..files).map(|n| format!("src/{n}.rs")).collect(),
        }
    });
    Facts {
        status,
        turn: tc.draw(gs::integers::<u32>().max_value(40)),
        asks: tc
            .draw(gs::booleans())
            .then(|| tc.draw(gs::from_regex(r"[A-Z][a-z ]{0,20}\?"))),
        forecast,
        landed: tc.draw(gs::booleans()),
    }
}

/// Every state `facts` allow, each with how pressing it is (lower wins).
fn allowed(facts: &Facts) -> Vec<(u8, Attention)> {
    let live = facts.status == Status::Live;
    let stopped = facts.status == Status::Stopped;
    let conflicts = facts
        .forecast
        .as_ref()
        .map(|forecast| forecast.conflicts.clone())
        .unwrap_or_default();
    let changes = facts.forecast.as_ref().map_or(0, |f| f.changes);
    let mut allowed = vec![(7, Attention::Idle)];
    if facts.landed {
        allowed.push((0, Attention::Landed));
    }
    if let (true, Some(question)) = (live, &facts.asks) {
        allowed.push((
            1,
            Attention::Asks {
                question: question.clone(),
            },
        ));
    }
    if live {
        allowed.push((2, Attention::Working { turn: facts.turn }));
    }
    if facts.status == Status::Interrupted {
        allowed.push((3, Attention::Interrupted));
    }
    if facts.status == Status::Failed {
        allowed.push((4, Attention::Failed));
    }
    if stopped && !conflicts.is_empty() {
        allowed.push((5, Attention::WouldConflict { files: conflicts }));
    }
    if stopped && changes > 0 {
        allowed.push((6, Attention::ReadyToLand { changes }));
    }
    allowed
}

/// A run's attention is the most pressing state its facts allow: landed
/// over everything, a question over work, a cut-off or failed run over
/// what landing it would do, a conflict over a clean landing.
#[hegel::test(test_cases = 500)]
fn attention_is_the_most_pressing_state_the_facts_allow(tc: TestCase) {
    let facts = tc.draw(facts().print_as_debug());
    let expected = allowed(&facts)
        .into_iter()
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, attention)| attention)
        .unwrap();
    let attention = Attention::of(&facts);
    assert_eq!(attention, expected);
    // The repository's pill counts the chats that ask, would conflict,
    // or are ready to land: nothing else.
    assert_eq!(
        attention.needs_you(),
        matches!(
            expected,
            Attention::Asks { .. }
                | Attention::WouldConflict { .. }
                | Attention::ReadyToLand { .. }
        )
    );
}

/// Each state's line under the title, as the canvas words it.
#[test]
fn each_state_reads_as_the_design_words_it() {
    let line = |attention: Attention| attention.line();
    assert_eq!(
        line(Attention::Working { turn: 7 }).as_deref(),
        Some("Working · turn 7")
    );
    assert_eq!(
        line(Attention::Asks {
            question: "Keep them?".into()
        })
        .as_deref(),
        Some("Asks you a question")
    );
    assert_eq!(
        line(Attention::ReadyToLand { changes: 2 }).as_deref(),
        Some("Ready to land · 2 changes")
    );
    assert_eq!(
        line(Attention::ReadyToLand { changes: 1 }).as_deref(),
        Some("Ready to land · 1 change")
    );
    assert_eq!(
        line(Attention::WouldConflict {
            files: vec!["a.rs".into(), "b.rs".into()]
        })
        .as_deref(),
        Some("Would conflict in 2 files")
    );
    assert_eq!(
        line(Attention::Interrupted).as_deref(),
        Some("Interrupted · tau closed")
    );
    for quiet in [Attention::Failed, Attention::Landed, Attention::Idle] {
        assert_eq!(line(quiet), None);
    }
}

/// A forecast with nothing to land leaves a stopped fork idle, and a
/// stored run's question, with the run no longer live, asks nothing.
#[test]
fn nothing_to_land_and_a_question_nobody_waits_on_are_idle() {
    let empty = Facts {
        status: Status::Stopped,
        forecast: Some(Forecast::default()),
        ..Facts::default()
    };
    assert_eq!(Attention::of(&empty), Attention::Idle);
    let stored = Facts {
        status: Status::Stopped,
        asks: Some("Keep them?".into()),
        ..Facts::default()
    };
    assert_eq!(Attention::of(&stored), Attention::Idle);
}
