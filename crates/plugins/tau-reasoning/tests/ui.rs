//! tau-reasoning's UI: what its fold makes of its records, what it says
//! as a run starts, and that it keeps to the design language.

use std::sync::Arc;

use hegel::generators as gs;
use serde_json::{Value, json};
use tau_reasoning::{
    NAME,
    Record,
    ui::{Note, ReasoningPlugin, State},
};
use tau_ui_plugin::{
    Fold as _,
    RunCtx,
    RunKind,
    Services,
    UiPlugin,
    testing::{FakeRun, run_ctx},
};

/// Folds `body` as the registry does: as one of the plugin's records.
fn fold(state: &mut State, body: &Value, run: &mut FakeRun) {
    state.apply(Record::parse(body).expect("a record"), run);
}

/// A choice as the plugin publishes it, on gpt-5.5's levels.
fn choice(chose: bool, effort: &str, runs_at: Option<&str>) -> Value {
    let levels: Vec<Value> = ["none", "low", "medium", "high", "xhigh"]
        .iter()
        .map(|effort| json!({ "effort": effort, "suits": "", "p": 0.2 }))
        .collect();
    json!({
        "kind": "choice",
        "verdict": if chose { "chose" } else { "kept" },
        "effort": effort, "confidence": 0.8, "threshold": 0.7,
        "levels": levels, "cost": 0.0, "runs_at": runs_at,
    })
}

/// Over any run of choices and failures, a note shows only where the
/// effort a message runs at changes, or a request failed; the plan and
/// the status say what the last choice left; every choice is kept.
#[hegel::test(test_cases = 300)]
fn notes_show_only_where_the_effort_changes(tc: hegel::TestCase) {
    let efforts = vec!["low", "medium", "high"];
    let steps: Vec<Option<(bool, &str, Option<&str>)>> = tc.draw(
        gs::vecs(gs::optional(hegel::tuples!(
            gs::booleans(),
            gs::sampled_from(efforts.clone()),
            gs::optional(gs::sampled_from(efforts.clone())),
        )))
        .max_size(8),
    );
    let mut state = State::default();
    let mut anchors = FakeRun::default();
    // The model: what each message ran at, and the notes it earns.
    let (mut ran_at, mut notes, mut choices) = (None::<String>, 0, 0);
    let mut last = None;
    for step in &steps {
        match step {
            None => {
                fold(
                    &mut state,
                    &json!({ "kind": "error", "message": "offline" }),
                    &mut anchors,
                );
                notes += 1;
            }
            Some((chose, effort, runs_at)) => {
                fold(
                    &mut state,
                    &choice(*chose, effort, *runs_at),
                    &mut anchors,
                );
                choices += 1;
                let now = runs_at
                    .map(str::to_owned)
                    .or_else(|| chose.then(|| effort.to_string()));
                if now != ran_at {
                    notes += 1;
                }
                ran_at = now.clone();
                last = Some((*chose, now));
            }
        }
    }
    assert_eq!(anchors.anchors.len(), notes, "{steps:?}");
    assert_eq!(state.notes.len(), notes);
    assert!(
        anchors
            .anchors
            .iter()
            .all(|key| state.notes.contains_key(key))
    );
    assert_eq!(state.choices.len(), choices);
    assert_eq!(state.ran_at, ran_at);
    if let Some((chose, now)) = last {
        assert_eq!(
            state.plan.as_deref(),
            Some(now.as_deref().unwrap_or("default"))
        );
        let status = state.status.clone().unwrap();
        match (chose, &now) {
            (true, Some(effort)) => {
                assert_eq!(status, format!("chose {effort}"))
            }
            (false, Some(effort)) => {
                assert_eq!(status, format!("stayed at {effort}"))
            }
            (_, None) => assert_eq!(status, "kept the default"),
        }
    }
}

/// The first pick says it picked; a change says from what to what.
#[test]
fn a_note_says_what_changed() {
    let mut state = State::default();
    let mut anchors = FakeRun::default();
    fold(&mut state, &choice(false, "low", None), &mut anchors);
    fold(
        &mut state,
        &choice(true, "high", Some("high")),
        &mut anchors,
    );
    fold(
        &mut state,
        &choice(false, "low", Some("high")),
        &mut anchors,
    );
    fold(&mut state, &choice(true, "low", Some("low")), &mut anchors);
    let texts: Vec<String> = anchors
        .anchors
        .iter()
        .map(|key| match &state.notes[key] {
            Note::Choice { text, .. } => text.clone(),
            Note::Failed { message, .. } => message.clone(),
        })
        .collect();
    assert_eq!(
        texts,
        [
            "picked **high** reasoning for this message",
            "reasoning **high** → **low**",
        ]
    );
}

/// An effort kept over Jev's pick to keep the cache gets a note even
/// when it does not change, saying what switching would have cost, and
/// the status says it was kept.
#[test]
fn a_kept_effort_says_what_it_saved() {
    let mut state = State::default();
    let mut anchors = FakeRun::default();
    fold(
        &mut state,
        &choice(true, "medium", Some("medium")),
        &mut anchors,
    );
    let mut kept = choice(true, "high", Some("medium"));
    kept["kept_for_cache"] = 48_213.into();
    fold(&mut state, &kept, &mut anchors);
    assert_eq!(anchors.anchors.len(), 2);
    let Note::Choice { text, outcome, .. } = &state.notes[&anchors.anchors[1]]
    else {
        panic!("a choice note");
    };
    assert_eq!(text, "kept **medium** reasoning for this message");
    assert_eq!(
        outcome,
        "but this message stays at medium: switching to high would \
         resend 48k tokens uncached."
    );
    assert_eq!(state.status.as_deref(), Some("kept medium"));
    assert_eq!(tau_reasoning::ui::tokens_text(900), "900");
    assert_eq!(tau_reasoning::ui::tokens_text(1_499), "1k");
    assert_eq!(tau_reasoning::ui::tokens_text(1_500), "2k");
}

fn run(effort: Option<&str>, jev: bool) -> RunCtx {
    let mut services = Services::default();
    if jev {
        let jev: Arc<dyn tau_jev::Jev> =
            Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.5));
        services = services.with(jev);
    }
    RunCtx {
        effort: effort.map(str::to_owned),
        services,
        ..run_ctx(RunKind::Chat)
    }
}

/// Property inventory: Every key/effort pair follows the startup status and
/// agent-plugin contract. The oracle is the explicit status branch plus the
/// expected plugin count/name; inputs are the fixed 2 × 8 Cartesian table,
/// with no rejection or shrinking so every contract case always runs.
#[test]
fn startup_status_and_plugin_presence_follow_key_and_effort() {
    let efforts = [
        None,
        Some("none"),
        Some("minimal"),
        Some("low"),
        Some("medium"),
        Some("high"),
        Some("xhigh"),
        Some("max"),
    ];
    let settings = Default::default();

    for jev in [false, true] {
        for effort in efforts {
            let run = run(effort, jev);
            let mut state = State::default();
            let mut anchors = FakeRun::default();
            for body in ReasoningPlugin.starting(&(), &run, &settings) {
                state.apply(body, &mut anchors);
            }
            let status = state.starting.unwrap();
            match (jev, effort) {
                (_, Some(effort)) => {
                    assert!(status.starts_with("off"), "{status}");
                    assert!(status.ends_with(effort), "{status}");
                }
                (false, None) => assert_eq!(status, tau_ui_plugin::NO_KEY),
                (true, None) => assert!(!status.starts_with("off"), "{status}"),
            }

            let should_auto_pick = jev && effort.is_none();
            let plugins =
                ReasoningPlugin.agent_plugins(&(), &run, &settings).unwrap();
            assert_eq!(plugins.len(), usize::from(should_auto_pick));
            assert!(plugins.iter().all(|plugin| plugin.name() == NAME));
        }
    }
}
