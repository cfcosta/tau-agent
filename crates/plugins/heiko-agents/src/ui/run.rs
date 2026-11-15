//! What an agent's conversation shows around it: a banner with where it
//! stands, a panel in the inspector, and its events as cards. Each is a
//! view tree (ADR 0045).

use heiko_ui_plugin::{
    ViewCx,
    points::{AtMessage, AtRun},
    tree::{Kind, MarkState, Node, Tone},
};
use serde_json::Value;

use super::{
    AgentsUi,
    DETAILS,
    SAVE_SECRET,
    SECRET_NAME_INPUT,
    SECRET_VALUE_INPUT,
    act_button,
    allowance,
    button,
    events,
    page::{MORE, ruled},
    state_mark,
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

/// The banner's tree: one line. On a phone the panel is in the run's
/// sheet: what the agent left the person opens it from here.
pub fn banner_tree(agent: &Agent, compact: bool) -> Node {
    let paused = agent.status == Status::Paused;
    let name = agent.name.clone();
    let mut line = format!("{} · {} effort", agent.model, agent.effort);
    if agent.queued > 0 {
        line.push_str(&format!(" · {} waiting", events(agent.queued)));
    }
    let mut head = vec![
        state_mark(&agent.status),
        Node::Mono {
            text: agent.name.clone(),
        },
        Node::text(agent.status.label()),
        Node::Toned {
            text: line,
            tone: Tone::Neutral,
            mono: true,
        },
        Node::Spacer,
    ];
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
    Node::Row { children: head }
}

/// The agent's panel in the inspector: what it left the person, how
/// much of the day's allowance it used, what wakes it, its keys and its
/// versions.
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
    let section =
        |id: &str, title: String, children: Vec<Node>| Node::Section {
            id: format!("agent-panel-{id}"),
            title,
            open: true,
            children,
        };
    let counted = |title: &str, count: usize| match count {
        0 => title.to_owned(),
        count => format!("{title} · {count}"),
    };
    let mut panel = Vec::new();
    let left = super::notices::rows(agent, false, draft);
    if !left.is_empty() {
        let title = counted("For you", left.len());
        panel.push(section("for-you", title, ruled(left)));
    }
    let mut today = allowance(agent).to_vec();
    if agent.status == Status::AtCap {
        today.push(Node::row([act_button(
            "Allow more today",
            Kind::Primary,
            &Act::AllowMore {
                agent: agent.name.clone(),
                amount: MORE,
            },
        )]));
    }
    panel.push(section("today", "Today".into(), today));
    let mut wakes: Vec<Node> = agent
        .triggers
        .iter()
        .map(|trigger| {
            // A stream says whether it is open; the rest, when they fire
            // next or fired last.
            let when = match (trigger.connection(), trigger.next, trigger.last)
            {
                (Some(connection), ..) => Some(Node::Toned {
                    text: connection.into(),
                    tone: if trigger.live == Some(true) {
                        Tone::Good
                    } else {
                        Tone::Neutral
                    },
                    mono: true,
                }),
                (None, Some(next), _) => Some(Node::Due {
                    id: format!("trigger-{}-{}-next", agent.name, trigger.name),
                    at: next * 1000,
                }),
                (None, None, Some(last)) => Some(ago(
                    format!("trigger-{}-{}-last", agent.name, trigger.name),
                    last,
                )),
                (None, None, None) => None,
            };
            let mut row = vec![
                kind(trigger.kind_label(), trigger.error.is_some()),
                Node::Mono {
                    text: trigger.label(),
                },
                Node::Spacer,
            ];
            row.extend(when);
            let mut rows = vec![Node::Row { children: row }];
            // A poll or a stream that failed says why, under it.
            if let Some(error) = &trigger.error {
                rows.push(Node::Toned {
                    text: error.clone(),
                    tone: Tone::Bad,
                    mono: true,
                });
            }
            Node::Stack { children: rows }
        })
        .collect();
    wakes.extend(scheduled(agent));
    let title = counted("What wakes it", wakes.len());
    if wakes.is_empty() {
        wakes.push(Node::Empty {
            text: "Nothing yet.".into(),
        });
    }
    panel.push(section("wakes", title, ruled(wakes)));
    panel.push(section(
        "keys",
        counted("Keys", agent.secrets.len()),
        secrets(agent),
    ));
    if !agent.versions.is_empty() {
        panel.push(section(
            "versions",
            "Versions".into(),
            ruled(agent.versions.iter().map(version_row).collect()),
        ));
    }
    if let Some(problem) = &agent.sandbox {
        panel.push(section(
            "commands",
            "Commands".into(),
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
            "Broken".into(),
            vec![Node::Status {
                text: error.clone(),
                detail: None,
                tone: Tone::Bad,
            }],
        ));
    }
    Node::Stack { children: panel }
}

/// What kind of thing wakes it, in mono: red when it is `failing`.
fn kind(label: &str, failing: bool) -> Node {
    Node::Toned {
        text: label.into(),
        tone: if failing { Tone::Bad } else { Tone::Info },
        mono: true,
    }
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

/// What `agent` scheduled for itself, soonest first: what and when, a
/// way for the person to drop it, and the note it left itself under.
fn scheduled(agent: &Agent) -> Vec<Node> {
    agent
        .scheduled
        .iter()
        .map(|scheduled| {
            let repeats = !matches!(scheduled.when, crate::When::Once { .. });
            Node::stack([
                Node::row([
                    kind(if repeats { "repeats" } else { "once" }, false),
                    Node::Mono {
                        text: if repeats {
                            format!(
                                "{} · {}",
                                scheduled.name,
                                scheduled.when.label()
                            )
                        } else {
                            scheduled.name.clone()
                        },
                    },
                    Node::Spacer,
                    Node::Due {
                        id: format!(
                            "scheduled-{}-{}",
                            agent.name, scheduled.name
                        ),
                        at: scheduled.next * 1000,
                    },
                    act_button(
                        "drop",
                        Kind::Danger,
                        &Act::Unschedule {
                            agent: agent.name.clone(),
                            name: scheduled.name.clone(),
                        },
                    ),
                ]),
                Node::Toned {
                    text: scheduled.note.clone(),
                    tone: Tone::Neutral,
                    mono: false,
                },
            ])
        })
        .collect()
}

/// The secrets `agent` names, a ruled row each, and a way to save one:
/// heiko's secret store holds the values, which never come back here.
fn secrets(agent: &Agent) -> Vec<Node> {
    let mut rows: Vec<Node> = ruled(
        agent
            .secrets
            .iter()
            .map(|secret| {
                let mut row = vec![
                    Node::Mark {
                        state: if secret.set {
                            MarkState::Met
                        } else {
                            MarkState::Asks
                        },
                        count: None,
                    },
                    Node::Mono {
                        text: secret.name.clone(),
                    },
                    Node::Spacer,
                ];
                row.push(if secret.set {
                    act_button(
                        "remove",
                        Kind::Danger,
                        &Act::RemoveSecret {
                            agent: agent.name.clone(),
                            name: secret.name.clone(),
                        },
                    )
                } else {
                    Node::Toned {
                        text: "missing".into(),
                        tone: Tone::Warn,
                        mono: true,
                    }
                });
                Node::Row { children: row }
            })
            .collect(),
    );
    if agent.secrets.is_empty() {
        rows.push(Node::Empty {
            text: "Its src/agent.luau names none yet.".into(),
        });
    }
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

/// A version of its `src/agent.luau`: live, failed, or one before.
fn version_row(version: &crate::Version) -> Node {
    let passed = version.tests.iter().filter(|test| test.passed).count();
    let (state, word) = if version.live {
        (MarkState::Met, "live")
    } else if version.error.is_some() {
        (MarkState::Failing, "never live")
    } else {
        (MarkState::Idle, "")
    };
    let short: String = version.commit.chars().take(8).collect();
    let mut row = vec![Node::row([
        Node::Mark { state, count: None },
        Node::text(version.summary.clone()),
        Node::Toned {
            text: short,
            tone: Tone::Neutral,
            mono: true,
        },
        Node::Spacer,
        Node::Mono { text: word.into() },
    ])];
    if !version.tests.is_empty() {
        row.push(Node::Toned {
            text: format!("{passed} of {} tests", version.tests.len()),
            tone: if passed == version.tests.len() {
                Tone::Good
            } else {
                Tone::Bad
            },
            mono: true,
        });
    }
    Node::Stack { children: row }
}

/// An event message, drawn as the batch it is: how many came and when,
/// then a ruled row each with its data under it.
pub(super) fn event_card(
    at: &AtMessage,
    _view: &mut ViewCx<'_, AgentsUi>,
) -> Option<Node> {
    Some(event_tree(&Message::parse(&at.text)?))
}

/// The event card's tree.
pub fn event_tree(message: &Message) -> Node {
    let mut card = vec![Node::row([
        Node::Rich {
            text: format!("**{}**", events(message.events.len())),
        },
        Node::Spacer,
        Node::Toned {
            text: message.span.clone(),
            tone: Tone::Neutral,
            mono: true,
        },
    ])];
    for event in &message.events {
        card.push(Node::Rule);
        card.push(Node::row([
            kind(event.kind.as_str(), false),
            Node::Mono {
                text: event.trigger.clone(),
            },
            Node::Toned {
                text: event.id.clone(),
                tone: Tone::Neutral,
                mono: true,
            },
            Node::Spacer,
            Node::Toned {
                text: event.time.clone(),
                tone: Tone::Neutral,
                mono: true,
            },
        ]));
        if !event.data.is_empty() && event.data != "null" {
            card.push(Node::Mono {
                text: event.data.clone(),
            });
        }
    }
    Node::Stack { children: card }
}
