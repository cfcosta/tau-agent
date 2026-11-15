//! What an agent left for the person (ADR 0037): each note, with a way
//! to dismiss it, and each question, with a way to answer it. Its panel
//! draws them as rows of a narrow column, the Agents page as a table's.

use heiko_ui_plugin::tree::{
    ButtonSize,
    Kind,
    MarkState,
    Node,
    Tone,
    draft_key,
};
use serde_json::json;

use super::{ANSWER, act_button, answer_input};
use crate::{Act, Agent, Notice};

/// One notice, in the parts a row is laid out from.
struct Left {
    mark: Node,
    /// Whether it asks or says, and how long ago.
    when: Vec<Node>,
    text: Node,
    /// What the person can do with it, the one that matters first.
    actions: Vec<Node>,
    /// The field of a question that takes any answer.
    field: Option<Node>,
    /// Whether it is a note: its only action dismisses it.
    note: bool,
}

/// `agent`'s notices, oldest first, in parts.
fn left(agent: &Agent, draft: &dyn Fn(&str) -> Option<String>) -> Vec<Left> {
    agent
        .notices
        .iter()
        .map(|notice| {
            let (actions, field) = actions(agent, notice, draft);
            Left {
                mark: Node::Mark {
                    state: if notice.asks() {
                        MarkState::Asks
                    } else {
                        MarkState::Idle
                    },
                    count: None,
                },
                when: vec![
                    Node::Toned {
                        text: if notice.asks() { "asks ·" } else { "says ·" }
                            .into(),
                        tone: Tone::Neutral,
                        mono: true,
                    },
                    Node::Ago {
                        id: format!("notice-{}-{}", agent.name, notice.id),
                        at: notice.at * 1000,
                    },
                ],
                text: Node::text(&*notice.text),
                actions,
                field,
                note: notice.choices.is_none(),
            }
        })
        .collect()
}

/// A row for each of `agent`'s notices, oldest first, for a narrow
/// column: its mark, the agent's name when `named`, whether it asks or
/// says and how long ago, then what it says, and under that what the
/// person can do with it. A note's "dismiss" is at its head's end. A
/// question with choices answers with one click, the first of them the
/// row's hot action; one without takes its own field, whose draft
/// `draft` gives by the field's UI state key.
pub fn rows(
    agent: &Agent,
    named: bool,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Vec<Node> {
    left(agent, draft)
        .into_iter()
        .map(|left| {
            let mut head = vec![left.mark];
            if named {
                head.push(Node::Rich {
                    text: format!("**{}**", agent.name),
                });
            }
            head.extend(left.when);
            // A note's way out is at its head's end; a question's
            // choices go under what it asks.
            let under = if left.note {
                head.push(Node::Spacer);
                head.extend(left.actions);
                Vec::new()
            } else {
                left.actions
            };
            let mut row = vec![Node::Row { children: head }, left.text];
            if !under.is_empty() {
                row.push(Node::Row { children: under });
            }
            row.extend(left.field);
            Node::Stack { children: row }
        })
        .collect()
}

/// A table row for each of `agent`'s notices, oldest first, for the
/// Agents page: the agent, whether it asks or says and how long ago,
/// what it says over its field, and what the person can do with it at
/// the row's end.
pub fn lines(
    agent: &Agent,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Vec<Vec<Node>> {
    left(agent, draft)
        .into_iter()
        .map(|left| {
            let mut said = vec![left.text];
            said.extend(left.field);
            let mut end = vec![Node::Spacer];
            end.extend(left.actions);
            vec![
                Node::row([
                    left.mark,
                    Node::Rich {
                        text: format!("**{}**", agent.name),
                    },
                ]),
                Node::Row {
                    children: left.when,
                },
                Node::Stack { children: said },
                Node::Row { children: end },
            ]
        })
        .collect()
}

/// What the person can do with `notice`: its actions, the first of a
/// question's choices the hot one and the rest links, and the field for
/// a question that takes any answer, with its primary "Answer".
fn actions(
    agent: &Agent,
    notice: &Notice,
    draft: &dyn Fn(&str) -> Option<String>,
) -> (Vec<Node>, Option<Node>) {
    let (name, key) = (agent.name.clone(), notice.id.clone());
    match &notice.choices {
        None => (
            vec![act_button(
                "dismiss",
                Kind::Secondary,
                &Act::Dismiss {
                    agent: name,
                    notice: key,
                },
            )],
            None,
        ),
        Some(choices) if !choices.is_empty() => (
            choices
                .iter()
                .enumerate()
                .map(|(n, choice)| {
                    act_button(
                        choice,
                        if n == 0 { Kind::Hot } else { Kind::Secondary },
                        &Act::Answer {
                            agent: name.clone(),
                            notice: key.clone(),
                            answer: choice.clone(),
                        },
                    )
                })
                .collect(),
            None,
        ),
        Some(_) => {
            let input = answer_input(&name, &key);
            let answer = draft(&draft_key(&input)).unwrap_or_default();
            let mut row = vec![Node::Input {
                id: input,
                placeholder: Some("Your answer".into()),
                multiline: false,
                submit: Some(format!("{ANSWER}:{name}/{key}")),
                keep: false,
                secret: false,
                focus: None,
            }];
            if !answer.trim().is_empty() {
                row.push(Node::Button {
                    label: "Answer".into(),
                    action: ANSWER.into(),
                    args: json!({ "agent": name, "notice": key, "answer": answer }),
                    kind: Kind::Primary,
                    icon: None,
                    size: ButtonSize::Medium,
                    wide: false,
                    detail: None,
                });
            }
            (Vec::new(), Some(Node::Row { children: row }))
        }
    }
}
