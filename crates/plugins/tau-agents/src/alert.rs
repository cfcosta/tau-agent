//! What reaches the person about their agents when they are not looking
//! (ADR 0037): a note or a question an agent left, and an agent that
//! spent its day. The desktop shows these as notifications, and so does
//! the paired phone; [`Watch`] decides when, for both.

use std::collections::{BTreeMap, BTreeSet};

use tau_ui_kit::format::usd;

use crate::{Agent, Overview, Status};

/// What an alert is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertKind {
    /// It left a note (`notify`).
    Note,
    /// It asked something (`ask`), and waits for the answer.
    Question,
    /// It spent what it may today: its events wait.
    AtCap,
}

/// One alert: the agent, its conversation, and the words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub agent: String,
    /// Its main chat, which opening the alert shows.
    pub main: Option<String>,
    pub kind: AlertKind,
    pub title: String,
    pub body: String,
}

/// Decides when to alert: once for each note or question an agent
/// leaves, and once each time it reaches its cap.
#[derive(Debug, Default)]
pub struct Watch {
    /// What each agent had when last seen.
    seen: BTreeMap<String, Seen>,
}

#[derive(Debug, Default)]
struct Seen {
    notices: BTreeSet<String>,
    at_cap: bool,
}

impl Watch {
    /// Takes in the agents as they are now, and says what is new since
    /// they were last seen. An agent seen for the first time alerts
    /// nothing: what it has was there before. One no longer there is
    /// forgotten.
    pub fn observe(&mut self, overview: &Overview) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let mut seen = BTreeMap::new();
        for agent in &overview.agents {
            let now = Seen {
                notices: agent
                    .notices
                    .iter()
                    .map(|notice| notice.id.clone())
                    .collect(),
                at_cap: agent.status == Status::AtCap,
            };
            if let Some(before) = self.seen.get(&agent.name) {
                alerts.extend(new_since(agent, before));
            }
            seen.insert(agent.name.clone(), now);
        }
        self.seen = seen;
        alerts
    }
}

/// What `agent` has that it did not have `before`.
fn new_since(agent: &Agent, before: &Seen) -> Vec<Alert> {
    let alert = |kind, title: String, body: String| Alert {
        agent: agent.name.clone(),
        main: agent.main.clone(),
        kind,
        title,
        body,
    };
    let mut alerts: Vec<Alert> = agent
        .notices
        .iter()
        .filter(|notice| !before.notices.contains(&notice.id))
        .map(|notice| {
            if notice.asks() {
                alert(
                    AlertKind::Question,
                    format!("{} asks you", agent.name),
                    notice.text.clone(),
                )
            } else {
                alert(
                    AlertKind::Note,
                    format!("{} says", agent.name),
                    notice.text.clone(),
                )
            }
        })
        .collect();
    if agent.status == Status::AtCap && !before.at_cap {
        alerts.push(alert(
            AlertKind::AtCap,
            format!("{} reached its cap", agent.name),
            format!(
                "It spent {} today; its events wait for tomorrow or for you",
                usd(agent.cap_today)
            ),
        ));
    }
    alerts
}
