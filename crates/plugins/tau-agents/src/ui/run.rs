//! What an agent's conversation shows around it: a banner with where it
//! stands, a panel in the inspector, and its events as cards. Each is a
//! view tree (ADR 0045).

use serde_json::Value;
use tau_ui_kit::format::usd;
use tau_ui_plugin::{
    ViewCx,
    points::{AtMessage, AtRun},
    tree::{Kind, Node, Tone},
};

use super::{
    AgentsUi,
    DETAILS,
    SAVE_SECRET,
    SECRET_NAME_INPUT,
    SECRET_VALUE_INPUT,
    act_button,
    button,
    status_pill,
    tree_tone,
};
use crate::{Act, Agent, Status, event::Message};

/// Above an agent's conversation: the agent, where it stands, its model,
/// and the way to pause it or let it go on.
pub(super) fn banner(
    at: &AtRun,
    view: &mut ViewCx<'_, AgentsUi>,
) -> Option<Node> {
    let agent = view
        .data
        .agent_of_main(&at.run.repo, &at.run.id.to_string())?;
    Some(banner_tree(agent, view.compact))
}

/// The banner's tree. On a phone the panel is in the run's sheet: what
/// the agent left the person opens it from here.
pub fn banner_tree(agent: &Agent, compact: bool) -> Node {
    let paused = agent.status == Status::Paused;
    let name = agent.name.clone();
    let mut head = vec![
        Node::Icon {
            name: "sub-agent".into(),
            tone: tree_tone(&agent.status),
        },
        Node::Mono {
            text: agent.name.clone(),
        },
        status_pill(&agent.status),
        Node::Spacer,
    ];
    if agent.queued > 0 {
        head.push(Node::text(match agent.queued {
            1 => "1 event waiting".to_owned(),
            n => format!("{n} events waiting"),
        }));
    }
    let waits = agent.waits_on_you();
    if compact && waits > 0 {
        head.push(button(
            &format!("{waits} for you"),
            Kind::Primary,
            DETAILS,
            Value::Null,
        ));
    }
    head.push(if paused {
        act_button("Resume", Kind::Secondary, &Act::Resume { agent: name })
    } else {
        act_button("Pause", Kind::Secondary, &Act::Pause { agent: name })
    });
    Node::Card {
        children: vec![
            Node::Row { children: head },
            Node::text(format!(
                "{} · {} effort · {} of {} today",
                agent.model,
                agent.effort,
                usd(agent.spend_today),
                usd(agent.cap_today),
            )),
        ],
    }
}

/// The agent's panel in the inspector: what it left the person, what it
/// spent today, what wakes it, its keys and its versions.
pub(super) fn panel(
    at: &AtRun,
    view: &mut ViewCx<'_, AgentsUi>,
) -> Option<Node> {
    let agent = view
        .data
        .agent_of_main(&at.run.repo, &at.run.id.to_string())?;
    let draft = |key: &str| {
        view.ui_value(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    Some(panel_tree(agent, &draft))
}

/// The panel's tree, its fields' drafts read with `draft`.
pub fn panel_tree(
    agent: &Agent,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Node {
    let section = |id: &str, title: &str, children: Vec<Node>| Node::Section {
        id: format!("agent-panel-{id}"),
        title: title.into(),
        open: true,
        children,
    };
    let mut panel = Vec::new();
    let left = super::notices::rows(agent, draft);
    if !left.is_empty() {
        panel.push(section("for-you", "For you", left));
    }
    let mut today = vec![
        Node::Progress {
            share: f64::from(agent.spent_share()),
            label: Some(format!(
                "{} of {}",
                usd(agent.spend_today),
                usd(agent.cap_today)
            )),
        },
        Node::text("At its cap it stops and waits. Only you can raise it."),
    ];
    if agent.status == Status::AtCap {
        today.push(Node::row([act_button(
            "Allow $1 more today",
            Kind::Primary,
            &Act::AllowMore {
                agent: agent.name.clone(),
                amount: 1.0,
            },
        )]));
    }
    panel.push(section("today", "Today", today));
    let mut wakes: Vec<Node> = agent
        .triggers
        .iter()
        .map(|trigger| {
            // A stream says whether it is open; the rest, when they fire
            // next or fired last.
            let when = match (trigger.connection(), trigger.next, trigger.last)
            {
                (Some(connection), ..) => Node::Pill {
                    text: connection.into(),
                    tone: if trigger.live == Some(true) {
                        Tone::Good
                    } else {
                        Tone::Neutral
                    },
                },
                (None, Some(next), _) => Node::Due {
                    id: format!("trigger-{}-{}-next", agent.name, trigger.name),
                    at: next * 1000,
                },
                (None, None, Some(last)) => ago(
                    format!("trigger-{}-{}-last", agent.name, trigger.name),
                    last,
                ),
                (None, None, None) => Node::text(""),
            };
            let mut rows = vec![Node::row([
                Node::Pill {
                    text: trigger.kind_label().into(),
                    tone: Tone::Neutral,
                },
                Node::text(trigger.label()),
                Node::Spacer,
                when,
            ])];
            // A poll or a stream that failed says why, under it.
            if let Some(error) = &trigger.error {
                rows.push(Node::Status {
                    text: error.clone(),
                    detail: None,
                    tone: Tone::Bad,
                });
            }
            Node::Stack { children: rows }
        })
        .collect();
    wakes.extend(scheduled(agent));
    if wakes.is_empty() {
        wakes.push(Node::text(
            "Nothing yet: it declares this in its agent.luau, or schedules \
             it from its conversation.",
        ));
    }
    panel.push(section("wakes", "What wakes it", wakes));
    panel.push(section("keys", "Keys", secrets(agent)));
    if !agent.versions.is_empty() {
        panel.push(section(
            "versions",
            "Versions",
            agent.versions.iter().map(version_row).collect(),
        ));
    }
    if let Some(problem) = &agent.sandbox {
        panel.push(section(
            "commands",
            "Commands",
            vec![Node::Status {
                text: problem.clone(),
                detail: None,
                tone: Tone::Warn,
            }],
        ));
    }
    if let Status::Broken { error } = &agent.status {
        panel.push(section(
            "broken",
            "Broken",
            vec![Node::Status {
                text: error.clone(),
                detail: None,
                tone: Tone::Bad,
            }],
        ));
    }
    Node::Stack { children: panel }
}

/// The time since `at`, in seconds since the epoch, counting: `4:12 ago`.
pub(super) fn ago(id: String, at: i64) -> Node {
    Node::row([
        Node::Ticker {
            id,
            since: at * 1000,
            until: None,
        },
        Node::Mono { text: "ago".into() },
    ])
}

/// What `agent` scheduled for itself, soonest first: what and when, the
/// note it left itself under, and a way for the person to cancel it.
fn scheduled(agent: &Agent) -> Vec<Node> {
    agent
        .scheduled
        .iter()
        .map(|scheduled| {
            let repeats = !matches!(scheduled.when, crate::When::Once { .. });
            let cancel = Node::Button {
                label: String::new(),
                action: super::ACT.into(),
                args: serde_json::to_value(Act::Unschedule {
                    agent: agent.name.clone(),
                    name: scheduled.name.clone(),
                })
                .unwrap_or(Value::Null),
                kind: Kind::Secondary,
                icon: Some("close".into()),
            };
            Node::stack([
                Node::row([
                    Node::Pill {
                        text: if repeats { "repeats" } else { "once" }.into(),
                        tone: Tone::Neutral,
                    },
                    Node::text(if repeats {
                        format!(
                            "{} · {}",
                            scheduled.name,
                            scheduled.when.label()
                        )
                    } else {
                        scheduled.name.clone()
                    }),
                    Node::Spacer,
                    Node::Due {
                        id: format!(
                            "scheduled-{}-{}",
                            agent.name, scheduled.name
                        ),
                        at: scheduled.next * 1000,
                    },
                    cancel,
                ]),
                Node::Mono {
                    text: scheduled.note.clone(),
                },
            ])
        })
        .collect()
}

/// The secrets `agent` names, by name, and a way to save one: tau's
/// secret store holds the values, which never come back here.
fn secrets(agent: &Agent) -> Vec<Node> {
    let mut rows: Vec<Node> = agent
        .secrets
        .iter()
        .map(|secret| {
            let mut row = vec![
                Node::Mono {
                    text: secret.name.clone(),
                },
                Node::Spacer,
                Node::Pill {
                    text: if secret.set { "set" } else { "missing" }.into(),
                    tone: if secret.set { Tone::Good } else { Tone::Warn },
                },
            ];
            if secret.set {
                row.push(act_button(
                    "Remove",
                    Kind::Secondary,
                    &Act::RemoveSecret {
                        agent: agent.name.clone(),
                        name: secret.name.clone(),
                    },
                ));
            }
            Node::Row { children: row }
        })
        .collect();
    if agent.secrets.is_empty() {
        rows.push(Node::text("Its agent.luau names none yet."));
    }
    rows.push(Node::text(
        "Kept in this agent's own secret store. Its requests name them; no \
         one reads them back.",
    ));
    rows.push(Node::Input {
        id: SECRET_NAME_INPUT.into(),
        placeholder: Some("github".into()),
        multiline: false,
        submit: None,
        keep: true,
        secret: false,
        focus: None,
    });
    // Drawn as dots, kept out of the shared UI state; Enter saves it.
    rows.push(Node::Input {
        id: SECRET_VALUE_INPUT.into(),
        placeholder: Some("its value, then Enter".into()),
        multiline: false,
        submit: Some(format!("{SAVE_SECRET}:{}", agent.name)),
        keep: false,
        secret: true,
        focus: None,
    });
    rows
}

/// A version of its `agent.luau`: live, failed, or one before.
fn version_row(version: &crate::Version) -> Node {
    let passed = version.tests.iter().filter(|test| test.passed).count();
    let (tone, state) = if version.live {
        (Tone::Good, "live")
    } else if version.error.is_some() {
        (Tone::Bad, "never live")
    } else {
        (Tone::Neutral, "")
    };
    let short: String = version.commit.chars().take(8).collect();
    let mut row = vec![Node::row([
        Node::Status {
            text: version.summary.clone(),
            detail: Some(short),
            tone,
        },
        Node::Spacer,
        Node::Mono { text: state.into() },
    ])];
    if !version.tests.is_empty() {
        row.push(Node::text(format!(
            "{passed} of {} tests pass",
            version.tests.len()
        )));
    }
    Node::Stack { children: row }
}

/// An event message, drawn as the batch it is: how many came and when,
/// then a line each with its data under it.
pub(super) fn event_card(
    at: &AtMessage,
    _view: &mut ViewCx<'_, AgentsUi>,
) -> Option<Node> {
    Some(event_tree(&Message::parse(&at.text)?))
}

/// The event card's tree.
pub fn event_tree(message: &Message) -> Node {
    let mut card = vec![Node::row([
        Node::Icon {
            name: "clock".into(),
            tone: Tone::Info,
        },
        Node::text(match message.events.len() {
            1 => "1 event".to_owned(),
            n => format!("{n} events"),
        }),
        Node::Spacer,
        Node::Mono {
            text: message.span.clone(),
        },
    ])];
    for event in &message.events {
        card.push(Node::Rule);
        card.push(Node::row([
            Node::Pill {
                text: event.kind.as_str().into(),
                tone: Tone::Neutral,
            },
            Node::text(event.trigger.clone()),
            Node::Mono {
                text: event.id.clone(),
            },
            Node::Spacer,
            Node::Mono {
                text: event.time.clone(),
            },
        ]));
        if !event.data.is_empty() && event.data != "null" {
            card.push(Node::Mono {
                text: event.data.clone(),
            });
        }
    }
    Node::Card { children: card }
}
