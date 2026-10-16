//! tau-memory's settings: luna at low effort unless the user says
//! otherwise, resolved against the plan's models.

use tau_ai::{
    model::{family_version, plan_models},
    responses::request::ReasoningEffort,
};
use tau_memory::ui::settings::{EFFORTS, Settings};

/// The newest model of `family` on the plan.
fn newest(family: &str) -> &'static tau_ai::model::Model {
    plan_models()
        .into_iter()
        .find(|model| {
            family_version(&model.id).is_some_and(|(f, _)| f == family)
        })
        .unwrap_or_else(|| panic!("no {family} on the plan"))
}

/// Every family and effort the pane offers resolves to the plan's
/// newest model of that family, with the effort when it takes it and
/// the model's default otherwise; auto is the model's default.
/// Finite inventory: 4 families × 4 efforts. Nothing saved is luna at
/// low, a pass after each run, and the pane says what it runs as.
#[test]
fn each_choice_resolves_against_the_plan() {
    let saved: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(
        (saved.family.as_str(), saved.reasoning.as_deref()),
        ("luna", Some("low"))
    );
    assert!(!saved.follow_chat);
    assert!(saved.after_each_run);
    let (luna, low) = saved.model().expect("luna is on the plan");
    assert_eq!(luna, newest("luna").id);
    assert_eq!(
        saved.runs_as(),
        format!(
            "Runs as {luna} · {}.",
            low.map_or("auto", ReasoningEffort::as_str)
        )
    );
    for family in tau_ai::model::PLAN_FAMILIES {
        for effort in EFFORTS {
            let settings = Settings {
                family: family.to_owned(),
                reasoning: effort.map(str::to_owned),
                ..Settings::default()
            };
            let model = newest(family);
            let expected = effort
                .and_then(ReasoningEffort::parse)
                .filter(|effort| model.efforts.contains(effort));
            assert_eq!(
                settings.model(),
                Some((model.id.clone(), expected)),
                "{family} {effort:?}"
            );
        }
    }
}

/// The chat's model, or a family the plan lacks: memory writes with the
/// run's own model, and the pane says so.
#[test]
fn the_chats_model_or_an_unknown_family_leaves_it_to_the_run() {
    let follow = Settings {
        follow_chat: true,
        ..Settings::default()
    };
    assert_eq!(follow.model(), None);
    assert_eq!(follow.runs_as(), "Runs as each chat's model.");
    let gone = Settings {
        family: "nova".into(),
        ..Settings::default()
    };
    assert_eq!(gone.model(), None);
    assert!(gone.runs_as().starts_with("No nova model on the plan"));
    // Saved settings round-trip, auto included.
    let auto = Settings {
        reasoning: None,
        ..Settings::default()
    };
    let saved = serde_json::to_string(&auto).unwrap();
    assert_eq!(serde_json::from_str::<Settings>(&saved).unwrap(), auto);
}
