//! When the person hears of their agents while they are away (ADR 0037):
//! once per note or question an agent leaves, once each time it reaches
//! its cap, and once, in one alert, for what was there when tau first
//! saw it.

use tau_agents::{
    Agent,
    Notice,
    Overview,
    Status,
    alert::{AlertKind, Watch},
};

fn agent(status: Status, notices: Vec<Notice>) -> Agent {
    Agent {
        name: "triage".into(),
        purpose: String::new(),
        status,
        triggers: Vec::new(),
        versions: Vec::new(),
        spend_today: 0.5,
        cap_today: 1.0,
        queued: 0,
        last: None,
        model: "gpt-6-luna".into(),
        effort: "low".into(),
        main: Some("main-of-triage".into()),
        secrets: Vec::new(),
        sandbox: None,
        notices,
        scheduled: Vec::new(),
    }
}

fn notice(id: &str, text: &str, choices: Option<Vec<String>>) -> Notice {
    Notice {
        id: id.into(),
        at: 0,
        text: text.into(),
        choices,
    }
}

fn of(agents: Vec<Agent>) -> Overview {
    Overview { agents }
}

#[test]
fn what_was_there_first_alerts_once_in_one_alert() {
    let mut watch = Watch::default();
    let first = of(vec![agent(
        Status::AtCap,
        vec![
            notice("n1", "old news", None),
            notice("n2", "Close #398?", Some(Vec::new())),
        ],
    )]);
    let alerts = watch.observe(&first);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, AlertKind::Question);
    assert_eq!(alerts[0].title, "triage needs you");
    assert_eq!(
        alerts[0].body,
        "While you were away: 1 question, 1 note, it spent its $1.00 for today"
    );
    assert_eq!(alerts[0].main.as_deref(), Some("main-of-triage"));
    assert!(watch.observe(&first).is_empty());
    // An agent with nothing waiting alerts nothing.
    let mut quiet = Watch::default();
    assert!(
        quiet
            .observe(&of(vec![agent(Status::Waiting, Vec::new())]))
            .is_empty()
    );
}

#[test]
fn each_new_note_and_question_alerts_once() {
    let mut watch = Watch::default();
    watch.observe(&of(vec![agent(Status::Waiting, Vec::new())]));
    let now = of(vec![agent(
        Status::Waiting,
        vec![
            notice("n1", "Build 2318 failed", None),
            notice("n2", "Quarantine it?", Some(vec!["Yes".into()])),
        ],
    )]);
    let alerts = watch.observe(&now);
    let said: Vec<(AlertKind, &str, &str)> = alerts
        .iter()
        .map(|alert| (alert.kind, alert.title.as_str(), alert.body.as_str()))
        .collect();
    assert_eq!(
        said,
        [
            (AlertKind::Note, "triage says", "Build 2318 failed"),
            (AlertKind::Question, "triage asks you", "Quarantine it?"),
        ]
    );
    assert_eq!(alerts[0].main.as_deref(), Some("main-of-triage"));
    assert!(watch.observe(&now).is_empty());
}

#[test]
fn reaching_the_cap_alerts_each_time_it_happens() {
    let mut watch = Watch::default();
    let waiting = of(vec![agent(Status::Waiting, Vec::new())]);
    let at_cap = of(vec![agent(Status::AtCap, Vec::new())]);
    watch.observe(&waiting);
    let alerts = watch.observe(&at_cap);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].kind, AlertKind::AtCap);
    assert_eq!(alerts[0].title, "triage reached its cap");
    assert!(watch.observe(&at_cap).is_empty());
    watch.observe(&waiting);
    assert_eq!(watch.observe(&at_cap).len(), 1);
}

#[test]
fn an_agent_gone_and_back_starts_over() {
    let mut watch = Watch::default();
    let with_note = of(vec![agent(
        Status::Waiting,
        vec![notice("n1", "hello", None)],
    )]);
    watch.observe(&of(vec![agent(Status::Waiting, Vec::new())]));
    watch.observe(&of(Vec::new()));
    // Back, it is seen for the first time again: what it has alerts
    // once, as at start, and then not again.
    assert_eq!(watch.observe(&with_note).len(), 1);
    assert!(watch.observe(&with_note).is_empty());
}
