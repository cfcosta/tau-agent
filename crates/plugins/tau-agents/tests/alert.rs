//! When the person hears of their agents while they are away (ADR 0037):
//! once per note or question an agent leaves, once each time it reaches
//! its cap, and once, in one alert, for what was there when tau first
//! saw it.

use std::collections::BTreeMap;

use hegel::generators::{self as gs, Generator as _};
use tau_agents::{
    Agent,
    Notice,
    Overview,
    Status,
    alert::{Alert, AlertKind, Watch},
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

/// The notices an agent may leave: a note or a question, by id.
const NOTICES: [(&str, bool); 4] =
    [("n1", false), ("q2", true), ("n3", false), ("q4", true)];

/// One agent as an overview shows it at one look, or gone.
#[derive(Debug, Clone)]
struct Look {
    present: bool,
    at_cap: bool,
    /// Which of [`NOTICES`] wait on the person.
    notices: Vec<bool>,
}

#[hegel::composite]
fn look(tc: &hegel::TestCase) -> Look {
    Look {
        present: tc.draw(gs::weighted_booleans(0.85)),
        at_cap: tc.draw(gs::booleans()),
        notices: tc.draw(
            gs::vecs(gs::weighted_booleans(0.4))
                .min_size(NOTICES.len())
                .max_size(NOTICES.len()),
        ),
    }
}

/// The agent `name` as `look` shows it.
fn shown(name: &str, look: &Look) -> Agent {
    let notices = NOTICES
        .iter()
        .zip(&look.notices)
        .filter(|(_, waits)| **waits)
        .map(|((id, asks), _)| {
            notice(id, &format!("{name} {id}"), asks.then(Vec::new))
        })
        .collect();
    let status = if look.at_cap {
        Status::AtCap
    } else {
        Status::Waiting
    };
    Agent {
        name: name.into(),
        main: Some(format!("main-of-{name}")),
        ..agent(status, notices)
    }
}

/// When the person hears of their agents (ADR 0037), against a model of
/// what each was last seen with. An agent seen for the first time (as tau
/// starts, or back after it was gone) alerts once for all it has
/// waiting, if anything: "<name> needs you", "While you were away: ",
/// then its questions, its notes and its spent cap, kind by the first of
/// those it has. After that, each new note says "<name> says" its text,
/// each new question "<name> asks you", in order, and reaching the cap
/// says so once each time it happens. An agent no longer shown is
/// forgotten.
#[hegel::test(test_cases = 300)]
fn alerts_follow_what_is_new_since_each_agent_was_last_seen(
    tc: hegel::TestCase,
) {
    const NAMES: [&str; 2] = ["triage", "digest"];
    let looks: Vec<Vec<Look>> = tc.draw(
        gs::vecs(gs::vecs(look()).min_size(2).max_size(2).print_as_debug())
            .max_size(6),
    );
    let mut watch = Watch::default();
    // What each agent had when last seen: its notices, and its cap.
    let mut seen: BTreeMap<&str, (Vec<bool>, bool)> = BTreeMap::new();
    for step in &looks {
        let agents: Vec<Agent> = NAMES
            .iter()
            .zip(step)
            .filter(|(_, look)| look.present)
            .map(|(name, look)| shown(name, look))
            .collect();
        let mut expected = Vec::new();
        let mut now = BTreeMap::new();
        for (name, look) in NAMES.iter().zip(step) {
            if !look.present {
                continue;
            }
            let alert = |kind, title: String, body: String| Alert {
                agent: (*name).to_owned(),
                main: Some(format!("main-of-{name}")),
                kind,
                title,
                body,
            };
            let waiting =
                NOTICES.iter().zip(&look.notices).filter(|(_, w)| **w);
            match seen.get(name) {
                None => {
                    let questions =
                        waiting.clone().filter(|((_, a), _)| *a).count();
                    let notes = waiting.count() - questions;
                    let mut parts = Vec::new();
                    for (count, one, many) in [
                        (questions, "question", "questions"),
                        (notes, "note", "notes"),
                    ] {
                        match count {
                            0 => {}
                            1 => parts.push(format!("1 {one}")),
                            n => parts.push(format!("{n} {many}")),
                        }
                    }
                    if look.at_cap {
                        parts.push("it spent its $1.00 for today".to_owned());
                    }
                    if !parts.is_empty() {
                        tc.event("first sight with something waiting");
                        let kind = if questions > 0 {
                            AlertKind::Question
                        } else if notes > 0 {
                            AlertKind::Note
                        } else {
                            AlertKind::AtCap
                        };
                        expected.push(alert(
                            kind,
                            format!("{name} needs you"),
                            format!(
                                "While you were away: {}",
                                parts.join(", ")
                            ),
                        ));
                    }
                }
                Some((before, was_at_cap)) => {
                    for ((id, asks), _) in waiting {
                        let at =
                            NOTICES.iter().position(|(n, _)| n == id).unwrap();
                        if before[at] {
                            continue;
                        }
                        tc.event("a new notice");
                        let (kind, title) = if *asks {
                            (AlertKind::Question, format!("{name} asks you"))
                        } else {
                            (AlertKind::Note, format!("{name} says"))
                        };
                        expected.push(alert(
                            kind,
                            title,
                            format!("{name} {id}"),
                        ));
                    }
                    if look.at_cap && !was_at_cap {
                        tc.event("reached the cap");
                        expected.push(alert(
                            AlertKind::AtCap,
                            format!("{name} reached its cap"),
                            "It spent $1.00 today; its events wait for \
                             tomorrow or for you"
                                .to_owned(),
                        ));
                    }
                }
            }
            now.insert(*name, (look.notices.clone(), look.at_cap));
        }
        seen = now;
        assert_eq!(watch.observe(&Overview { agents }), expected);
    }
}
