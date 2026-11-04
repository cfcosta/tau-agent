//! What reaches the person about their agents when they are not looking
//! (ADR 0037): a note or a question an agent left, and an agent that
//! spent its day. The desktop shows these as notifications, and so does
//! the paired phone; tau-ui-plugin's `Watch` decides when, for both.

use tau_agent::tool::RunId;
use tau_ui_kit::format::usd;
use tau_ui_plugin::{WaitGroup, WaitItem, Waiting};

use crate::{Agent, Overview, Status};

/// What waits on the person among the agents: each one's notes and
/// questions, and its spent day, as [`tau_ui_plugin::points::WAITING`]
/// lists them.
pub fn waiting(overview: &Overview) -> Waiting {
    Waiting {
        groups: overview
            .agents
            .iter()
            .map(|agent| WaitGroup {
                name: agent.name.clone(),
                run: agent.main.clone().map(|main| RunId(main.into())),
                items: items(agent),
                away: away(agent),
            })
            .collect(),
    }
}

/// Each thing of `agent`'s that waits on the person: its notes and
/// questions by their ids, then its cap while it is spent.
fn items(agent: &Agent) -> Vec<WaitItem> {
    let mut items: Vec<WaitItem> = agent
        .notices
        .iter()
        .map(|notice| WaitItem {
            id: notice.id.clone(),
            title: if notice.asks() {
                format!("{} asks you", agent.name)
            } else {
                format!("{} says", agent.name)
            },
            body: notice.text.clone(),
        })
        .collect();
    if agent.status == Status::AtCap {
        items.push(WaitItem {
            id: AT_CAP.into(),
            title: format!("{} reached its cap", agent.name),
            body: format!(
                "It spent {} today; its events wait for tomorrow or for you",
                usd(agent.cap_today)
            ),
        });
    }
    items
}

/// The id of an agent's spent day among what waits.
const AT_CAP: &str = "at-cap";

/// One notice for all `agent` has waiting, when first seen: its
/// questions, its notes, its cap.
fn away(agent: &Agent) -> Option<WaitItem> {
    let questions = agent.notices.iter().filter(|notice| notice.asks()).count();
    let notes = agent.notices.len() - questions;
    let at_cap = agent.status == Status::AtCap;
    let count = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let mut parts: Vec<String> = [
        count(questions, "question", "questions"),
        count(notes, "note", "notes"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if at_cap {
        parts.push(format!("it spent its {} for today", usd(agent.cap_today)));
    }
    if parts.is_empty() {
        return None;
    }
    Some(WaitItem {
        id: String::new(),
        title: format!("{} needs you", agent.name),
        body: format!("While you were away: {}", parts.join(", ")),
    })
}
