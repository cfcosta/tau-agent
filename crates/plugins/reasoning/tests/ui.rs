//! tau-reasoning's UI: what its fold makes of its records, what it says
//! as a run starts, and that it keeps to the design language.

use std::sync::Arc;

use hegel::generators as gs;
use serde_json::{Value, json};
use tau_reasoning::{
    NAME,
    ui::{Note, ReasoningPlugin, State},
};
use tau_ui_plugin::{
    CardInfo,
    RepoCtx,
    RunCtx,
    RunCx,
    RunKind,
    Services,
    UiPlugin,
};

/// The anchors a fold places, in order.
#[derive(Default)]
struct Anchors(Vec<String>);

impl RunCx for Anchors {
    fn transcript(&mut self, key: &str) {
        self.0.push(key.to_owned());
    }

    fn attach(&mut self, _: &str, _: &str) -> bool {
        false
    }

    fn dropped(&mut self, _: &str, _: tau_ui_plugin::Dropped) -> bool {
        false
    }

    fn cut(&mut self, _: &str, _: tau_ui_plugin::OutputCut) -> bool {
        false
    }

    fn rewrite(&mut self, _: &str) {}

    fn cards(&self) -> Vec<CardInfo> {
        Vec::new()
    }

    fn last_text(&self) -> Option<String> {
        None
    }

    fn turn(&self) -> u32 {
        0
    }
}

/// A choice as the plugin publishes it, on gpt-5.5's levels.
fn choice(chose: bool, effort: &str, runs_at: Option<&str>) -> Value {
    let levels: Vec<Value> = ["none", "low", "medium", "high", "xhigh"]
        .iter()
        .map(|effort| json!({ "effort": effort, "suits": "", "p": 0.2 }))
        .collect();
    json!({
        "kind": if chose { "chose" } else { "kept" },
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
    let mut anchors = Anchors::default();
    // The model: what each message ran at, and the notes it earns.
    let (mut ran_at, mut notes, mut choices) = (None::<String>, 0, 0);
    let mut last = None;
    for step in &steps {
        match step {
            None => {
                state.apply(
                    &json!({ "kind": "error", "message": "offline" }),
                    &mut anchors,
                );
                notes += 1;
            }
            Some((chose, effort, runs_at)) => {
                state.apply(&choice(*chose, effort, *runs_at), &mut anchors);
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
    assert_eq!(anchors.0.len(), notes, "{steps:?}");
    assert_eq!(state.notes.len(), notes);
    assert!(anchors.0.iter().all(|key| state.notes.contains_key(key)));
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
    let mut anchors = Anchors::default();
    state.apply(&choice(false, "low", None), &mut anchors);
    state.apply(&choice(true, "high", Some("high")), &mut anchors);
    state.apply(&choice(false, "low", Some("high")), &mut anchors);
    state.apply(&choice(true, "low", Some("low")), &mut anchors);
    let texts: Vec<String> = anchors
        .0
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

fn run(effort: Option<&str>, jev: bool) -> RunCtx {
    let mut services = Services::default();
    if jev {
        let jev: Arc<dyn tau_jev::Jev> =
            Arc::new(tau_jev::fake::FakeJev::nouls(|_| 0.5));
        services = services.with(jev);
    }
    RunCtx {
        kind: RunKind::Chat,
        repo: RepoCtx {
            name: "repo".into(),
            checkout: "/tmp/repo".into(),
            dir: "/tmp/tau/repo".into(),
        },
        model: "gpt-5.5".into(),
        effort: effort.map(str::to_owned),
        services,
    }
}

/// As a run starts, tau-reasoning says it is off without a key or when
/// the effort was picked by hand, and builds its agent plugin only
/// otherwise.
#[hegel::test(test_cases = 50)]
fn it_says_whether_it_is_on(tc: hegel::TestCase) {
    let jev = tc.draw(gs::booleans());
    let effort = tc.draw(gs::optional(gs::sampled_from(vec!["low", "high"])));
    let run = run(effort, jev);
    let settings = Default::default();
    let bodies = ReasoningPlugin.starting(&(), &run, &settings);
    let mut state = State::default();
    for body in &bodies {
        state.apply(body, &mut Anchors::default());
    }
    let status = state.starting.unwrap();
    let on = jev && effort.is_none();
    assert_eq!(!status.starts_with("off"), on, "{status}");
    if let Some(effort) = effort {
        assert!(status.ends_with(effort), "{status}");
    } else if !jev {
        assert_eq!(status, tau_ui_plugin::NO_KEY);
    }
    let plugins = ReasoningPlugin.agent_plugins(&(), &run, &settings);
    assert_eq!(plugins.len(), usize::from(on));
    assert!(plugins.iter().all(|plugin| plugin.name() == NAME));
}

/// The UI takes its look from the kit.
#[test]
fn only_the_kit_holds_design_values() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
