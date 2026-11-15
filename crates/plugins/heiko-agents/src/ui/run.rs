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
    allowance_share,
    allowance_ticks,
    button,
    events,
    gap,
    page::ruled,
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
            Kind::Hot,
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
    // A section: its head, with a count when there is one, over its
    // rows.
    let section = |title: &str, count: usize, children: Vec<Node>| {
        let mut section = vec![Node::Header {
            title: title.to_owned(),
            meta: (count > 0).then(|| count.to_string()),
            actions: vec![],
        }];
        section.extend(children);
        Node::Stack { children: section }
    };
    let mut panel = Vec::new();
    let left = super::notices::rows(agent, false, draft);
    if !left.is_empty() {
        panel.push(section("For you", left.len(), ruled(left)));
        panel.push(gap());
    }
    // Today: the share of the allowance at the head's end, over its
    // strip.
    let mut today = vec![
        Node::Header {
            title: "Today".into(),
            meta: Some(format!(
                "{} of today's allowance",
                allowance_share(agent)
            )),
            actions: vec![],
        },
        allowance_ticks(agent),
    ];
    if agent.status == Status::AtCap {
        today.push(Node::row([act_button(
            super::ALLOW_MORE,
            Kind::Hot,
            &Act::AllowMore {
                agent: agent.name.clone(),
            },
        )]));
    }
    panel.push(Node::Stack { children: today });
    let wakes = wakes(agent, PANEL_ROOM);
    panel.push(gap());
    panel.push(section(
        "What wakes it",
        agent.triggers.len() + agent.scheduled.len(),
        wakes,
    ));
    panel.push(gap());
    panel.push(section("Keys", agent.secrets.len(), secrets(agent)));
    if !agent.versions.is_empty() {
        panel.push(gap());
        panel.push(section(
            "Versions",
            0,
            ruled(agent.versions.iter().map(version_row).collect()),
        ));
    }
    if let Some(problem) = &agent.sandbox {
        panel.push(gap());
        panel.push(section(
            "Commands",
            0,
            vec![Node::Status {
                text: problem.clone(),
                detail: None,
                tone: Tone::Warn,
            }],
        ));
    }
    if let Status::Broken { error } = &agent.status {
        panel.push(gap());
        panel.push(section(
            "Broken",
            0,
            vec![Node::Status {
                text: error.clone(),
                detail: None,
                tone: Tone::Bad,
            }],
        ));
    }
    Node::Stack { children: panel }
}

/// How many characters wide the kind of a thing that wakes the agent
/// is set: the longest, `schedule`.
const KIND_WIDTH: usize = 8;

/// How many characters of what wakes it a row of the narrow panel has
/// room for, beside its kind and when.
const PANEL_ROOM: usize = 22;

/// The same on a page of its own.
pub(super) const PAGE_ROOM: usize = 96;

/// How many of those characters a row's "drop" takes.
const DROP_ROOM: usize = 6;

/// What kind of thing wakes it, in mono: red when it is `failing`.
fn kind(label: &str, failing: bool) -> Node {
    Node::Toned {
        text: label.into(),
        tone: if failing { Tone::Bad } else { Tone::Info },
        mono: true,
    }
}

/// The kind at the head of a row of what wakes the agent: on one line,
/// filled to [`KIND_WIDTH`] so every row's text starts in one column.
fn wake_kind(label: &str, failing: bool) -> Node {
    Node::Pill {
        text: format!("{label:\u{a0}<width$}", width = KIND_WIDTH),
        tone: if failing { Tone::Bad } else { Tone::Info },
    }
}

/// `text`, cut to `room` characters, the last a mark that it goes on.
fn clipped(text: &str, room: usize) -> String {
    if text.chars().count() <= room {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(room.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// One thing that wakes the agent: a line of its kind, what it is, cut
/// to `room` characters, and `end` at the right; then `under`, each a
/// line of its own.
fn wake(
    kind: Node,
    what: &str,
    room: usize,
    end: Vec<Node>,
    under: Vec<Node>,
) -> Node {
    let mut row = vec![
        kind,
        // One line, in mono: it never breaks inside a word.
        Node::Marked {
            text: clipped(what, room),
            spans: vec![],
        },
        Node::Spacer,
    ];
    row.extend(end);
    let mut lines = vec![Node::Row { children: row }];
    lines.extend(under);
    Node::Stack { children: lines }
}

/// What wakes `agent`, a ruled row each: its triggers, then what it
/// scheduled for itself. `room` is how many characters of what each is
/// a row shows.
pub(super) fn wakes(agent: &Agent, room: usize) -> Vec<Node> {
    let mut rows: Vec<Node> = agent
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
                (None, None, Some(last)) => Some(Node::Ago {
                    id: format!("trigger-{}-{}-last", agent.name, trigger.name),
                    at: last * 1000,
                }),
                (None, None, None) => None,
            };
            // A poll or a stream that failed says why, under it.
            let error = trigger.error.as_ref().map(|error| Node::Toned {
                text: error.clone(),
                tone: Tone::Bad,
                mono: true,
            });
            wake(
                wake_kind(trigger.kind_label(), trigger.error.is_some()),
                &trigger.label(),
                room,
                when.into_iter().collect(),
                error.into_iter().collect(),
            )
        })
        .collect();
    rows.extend(scheduled(agent, room));
    if rows.is_empty() {
        return vec![Node::Empty {
            text: "Nothing yet.".into(),
        }];
    }
    ruled(rows)
}

/// What `agent` scheduled for itself, soonest first, a row each:
/// whether it repeats, its name, and when, with a way for the person to
/// drop it; under it, when it repeats and the note it left itself.
fn scheduled(agent: &Agent, room: usize) -> Vec<Node> {
    agent
        .scheduled
        .iter()
        .map(|scheduled| {
            let repeats = !matches!(scheduled.when, crate::When::Once { .. });
            let note = match (repeats, scheduled.note.is_empty()) {
                (true, true) => scheduled.when.label(),
                (true, false) => {
                    format!("{} · {}", scheduled.when.label(), scheduled.note)
                }
                (false, _) => scheduled.note.clone(),
            };
            let under = (!note.is_empty()).then(|| Node::Toned {
                text: note,
                tone: Tone::Neutral,
                mono: false,
            });
            wake(
                wake_kind(if repeats { "repeats" } else { "once" }, false),
                &scheduled.name,
                room.saturating_sub(DROP_ROOM),
                vec![
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
                ],
                under.into_iter().collect(),
            )
        })
        .collect()
}

/// The secrets `agent` names, a ruled row each, and a way to save one:
/// heiko's secret store holds the values, which never come back here.
pub(super) fn secrets(agent: &Agent) -> Vec<Node> {
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
/// then a ruled row each: its kind, what came with its data, and when.
pub(super) fn event_card(
    at: &AtMessage,
    _view: &mut ViewCx<'_, AgentsUi>,
) -> Option<Node> {
    Some(event_tree(&Message::parse(&at.text)?))
}

/// The shares of the card's width an event gives its kind, what came,
/// and when.
const EVENT: [f32; 3] = [1., 7., 1.4];

/// The event card's tree.
pub fn event_tree(message: &Message) -> Node {
    let head = Node::row([
        Node::Rich {
            text: format!("**{}**", events(message.events.len())),
        },
        Node::Spacer,
        Node::Toned {
            text: message.span.clone(),
            tone: Tone::Neutral,
            mono: true,
        },
    ]);
    let rows = message
        .events
        .iter()
        .map(|event| {
            let mut what = vec![
                Node::Mono {
                    text: event.trigger.clone(),
                },
                Node::Toned {
                    text: event.id.clone(),
                    tone: Tone::Neutral,
                    mono: true,
                },
            ];
            if !event.data.is_empty() && event.data != "null" {
                what.push(Node::Mono {
                    text: event.data.clone(),
                });
            }
            vec![
                kind(event.kind.as_str(), false),
                Node::Row { children: what },
                Node::row([
                    Node::Spacer,
                    Node::Toned {
                        text: event.time.clone(),
                        tone: Tone::Neutral,
                        mono: true,
                    },
                ]),
            ]
        })
        .collect();
    Node::stack([
        head,
        Node::Table {
            head: vec![],
            rows,
            widths: EVENT.to_vec(),
        },
    ])
}
