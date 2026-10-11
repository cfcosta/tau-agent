//! How the interface words an agent: times relative to now, its
//! triggers, a name from its purpose, and its commands.

use hegel::generators as gs;
use tau_agents::{
    Agent,
    Overview,
    Status,
    Trigger,
    TriggerKind,
    shown::{relative, suggest_name},
    ui::commands,
    valid_name,
};

fn trigger(kind: TriggerKind) -> Trigger {
    Trigger {
        name: "t".into(),
        kind,
        last: None,
        next: None,
        error: None,
    }
}

fn agent(name: &str, triggers: Vec<Trigger>) -> Agent {
    Agent {
        name: name.into(),
        purpose: String::new(),
        status: Status::Waiting,
        triggers,
        versions: Vec::new(),
        spend_today: 0.5,
        cap_today: 1.0,
        queued: 0,
        last: None,
        model: "gpt-6-luna".into(),
        effort: "low".into(),
        main: None,
        secrets: Vec::new(),
    }
}

#[test]
fn times_are_said_from_now() {
    assert_eq!(relative(1000, 1000), "now");
    assert_eq!(relative(1000 + 360, 1000), "in 6m");
    assert_eq!(relative(1000 - 240, 1000), "4m ago");
    assert_eq!(relative(1000 + 7200, 1000), "in 2h");
    assert_eq!(relative(1000 - 3 * 86_400, 1000), "3d ago");
}

#[hegel::test]
fn a_time_reads_as_future_or_past_by_its_side_of_now(tc: hegel::TestCase) {
    let now: i64 =
        tc.draw(gs::integers::<i64>().min_value(0).max_value(1 << 40));
    let gap: i64 = tc.draw(
        gs::integers::<i64>()
            .min_value(-(1 << 30))
            .max_value(1 << 30),
    );
    let said = relative(now + gap, now);
    match gap {
        -59..=59 => assert_eq!(said, "now"),
        60.. => assert!(said.starts_with("in "), "{said}"),
        _ => assert!(said.ends_with(" ago"), "{said}"),
    }
}

#[test]
fn triggers_say_what_wakes_it() {
    let schedule = trigger(TriggerKind::Schedule {
        every: None,
        at: Some("09:00".into()),
        days: vec!["Mon".into(), "Fri".into()],
    });
    assert_eq!(schedule.label(), "09:00 Mon Fri");
    assert_eq!(schedule.kind_label(), "schedule");
    let every = trigger(TriggerKind::Schedule {
        every: Some("15m".into()),
        at: None,
        days: Vec::new(),
    });
    assert_eq!(every.label(), "every 15m");
    let command = trigger(TriggerKind::Command {
        command: "triage".into(),
        hint: Some("<number>".into()),
    });
    assert_eq!(command.label(), "/triage <number>");
}

#[test]
fn a_name_comes_from_the_purpose() {
    assert_eq!(
        suggest_name("Watch the RustSec advisories feed every morning"),
        "watch-rustsec-advisories"
    );
    assert_eq!(suggest_name("!!!"), "agent");
    assert_eq!(suggest_name("42 things"), "agent");
}

#[hegel::test]
fn a_suggested_name_is_always_valid(tc: hegel::TestCase) {
    let purpose: String = tc.draw(gs::text().max_size(200));
    assert!(valid_name(&suggest_name(&purpose)));
}

#[test]
fn every_command_trigger_is_a_composer_command() {
    let overview = Overview {
        agents: vec![
            agent(
                "pr-triage",
                vec![
                    trigger(TriggerKind::Command {
                        command: "triage".into(),
                        hint: Some("<number>".into()),
                    }),
                    trigger(TriggerKind::Schedule {
                        every: Some("1h".into()),
                        at: None,
                        days: Vec::new(),
                    }),
                ],
            ),
            agent(
                "digest",
                vec![trigger(TriggerKind::Command {
                    command: "digest".into(),
                    hint: None,
                })],
            ),
        ],
    };
    let listed: Vec<(String, String, String)> = commands(&overview)
        .into_iter()
        .map(|command| (command.name, command.args, command.hint))
        .collect();
    assert_eq!(
        listed,
        [
            ("triage".into(), "<number>".into(), "Tell pr-triage".into()),
            ("digest".into(), String::new(), "Tell digest".into()),
        ]
    );
}

#[test]
fn an_agent_needs_the_person_at_its_cap_or_broken() {
    let mut agent = agent("a", Vec::new());
    assert!(!agent.needs_you());
    agent.status = Status::AtCap;
    assert!(agent.needs_you());
    agent.status = Status::Broken {
        error: "no agent.luau".into(),
    };
    assert!(agent.needs_you());
    agent.spend_today = 3.0;
    assert_eq!(agent.spent_share(), 1.0);
}
