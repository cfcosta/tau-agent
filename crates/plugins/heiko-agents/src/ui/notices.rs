//! What an agent left for the person (ADR 0037): each note, with a way
//! to dismiss it, and each question, with a way to answer it. Its panel
//! and the Agents page draw the same rows.

use heiko_ui_plugin::tree::{Kind, MarkState, Node, draft_key};
use serde_json::json;

use super::{ANSWER, act_button, answer_input, button};
use crate::{Act, Agent, Notice};

/// A row for each of `agent`'s notices, oldest first: its mark, the
/// agent's name when `named`, whether it asks or says and how long ago,
/// and what the person can do with it, then what it says. A question
/// with choices answers with one click, the first of them the row's
/// block; one without takes its own field, whose draft `draft` gives by
/// the field's UI state key.
pub fn rows(
    agent: &Agent,
    named: bool,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Vec<Node> {
    agent
        .notices
        .iter()
        .map(|notice| {
            let mut head = vec![Node::Mark {
                state: if notice.asks() {
                    MarkState::Asks
                } else {
                    MarkState::Idle
                },
                count: None,
            }];
            if named {
                head.push(Node::Rich {
                    text: format!("**{}**", agent.name),
                });
            }
            head.extend([
                Node::Mono {
                    text: if notice.asks() { "asks" } else { "says" }.into(),
                },
                Node::Ticker {
                    id: format!("notice-{}-{}", agent.name, notice.id),
                    since: notice.at * 1000,
                    until: None,
                },
                Node::Mono { text: "ago".into() },
                Node::Spacer,
            ]);
            let (beside, under) = actions(agent, notice, draft);
            head.extend(beside);
            let mut row =
                vec![Node::Row { children: head }, Node::text(&*notice.text)];
            row.extend(under);
            Node::Stack { children: row }
        })
        .collect()
}

/// What the person can do with `notice`: the actions beside its head,
/// and the field under it for a question that takes any answer.
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
                        if n == 0 {
                            Kind::Primary
                        } else {
                            Kind::Secondary
                        },
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
                row.push(button(
                    "Answer",
                    Kind::Primary,
                    ANSWER,
                    json!({ "agent": name, "notice": key, "answer": answer }),
                ));
            }
            (Vec::new(), Some(Node::Row { children: row }))
        }
    }
}
