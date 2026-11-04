//! What an agent left for the person (ADR 0037): each note, with a way
//! to dismiss it, and each question, with a way to answer it. Its panel
//! and the Agents page draw the same rows.

use serde_json::json;
use tau_ui_plugin::tree::{Kind, Node, draft_key};

use super::{ANSWER, act_button, answer_input, button};
use crate::{Act, Agent, Notice};

/// A row for each of `agent`'s notices, oldest first: what it says and
/// how long ago, then what the person can do with it. A question with
/// choices answers with one click; one without takes its own field,
/// whose draft `draft` gives by the field's UI state key.
pub fn rows(
    agent: &Agent,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Vec<Node> {
    agent
        .notices
        .iter()
        .map(|notice| {
            Node::stack([
                Node::row([
                    Node::Mono {
                        text: if notice.asks() { "asks" } else { "says" }
                            .into(),
                    },
                    Node::Ticker {
                        id: format!("notice-{}-{}", agent.name, notice.id),
                        since: notice.at * 1000,
                        until: None,
                    },
                    Node::Mono { text: "ago".into() },
                ]),
                Node::text(notice.text.clone()),
                actions(agent, notice, draft),
            ])
        })
        .collect()
}

/// What the person can do with `notice`.
fn actions(
    agent: &Agent,
    notice: &Notice,
    draft: &dyn Fn(&str) -> Option<String>,
) -> Node {
    let (name, key) = (agent.name.clone(), notice.id.clone());
    match &notice.choices {
        None => Node::row([act_button(
            "Dismiss",
            Kind::Secondary,
            &Act::Dismiss {
                agent: name,
                notice: key,
            },
        )]),
        Some(choices) if !choices.is_empty() => {
            Node::row(choices.iter().enumerate().map(|(n, choice)| {
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
            }))
        }
        Some(_) => {
            let input = answer_input(&name, &key);
            let answer = draft(&draft_key(&input)).unwrap_or_default();
            let mut row = vec![Node::Input {
                id: input,
                placeholder: Some("Your answer".into()),
                multiline: false,
                submit: Some(format!("{ANSWER}:{name}/{key}")),
                keep: false,
            }];
            if !answer.trim().is_empty() {
                row.push(button(
                    "Answer",
                    Kind::Primary,
                    ANSWER,
                    json!({ "agent": name, "notice": key, "answer": answer }),
                ));
            }
            Node::Row { children: row }
        }
    }
}
