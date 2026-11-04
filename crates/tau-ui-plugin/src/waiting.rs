//! When the person hears of what waits on them while they are away (ADR
//! 0037): plugins list it at [`crate::points::WAITING`], and [`Watch`]
//! decides when a notice is due, for the computer's window and the
//! phone's alike.

use std::collections::{BTreeMap, BTreeSet};

use tau_agent::tool::RunId;

use crate::{WaitItem, Waiting};

/// One notice: the plugin and its group it is of, the run opening it
/// shows, and its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub plugin: String,
    /// What it is of, by name: an agent's.
    pub group: String,
    pub run: Option<RunId>,
    pub title: String,
    pub body: String,
}

/// Decides when to tell: once for each thing that starts waiting. A
/// group seen for the first time, as tau starts, tells once of all it
/// has waiting, in its `away` words: it was left while no one was told.
/// A thing that no longer waits is forgotten, so it tells again if it
/// waits again; so is a group no longer listed.
#[derive(Debug, Default)]
pub struct Watch {
    /// What each plugin's group had waiting when last seen.
    seen: BTreeMap<(String, String), BTreeSet<String>>,
}

impl Watch {
    /// Takes in what waits now, by plugin, and says what is new since it
    /// was last seen.
    pub fn observe(&mut self, waiting: &[(&str, Waiting)]) -> Vec<Alert> {
        let mut alerts = Vec::new();
        let mut seen = BTreeMap::new();
        for (plugin, waiting) in waiting {
            for group in &waiting.groups {
                let key = ((*plugin).to_owned(), group.name.clone());
                let alert = |item: &WaitItem| Alert {
                    plugin: (*plugin).to_owned(),
                    group: group.name.clone(),
                    run: group.run.clone(),
                    title: item.title.clone(),
                    body: item.body.clone(),
                };
                match self.seen.get(&key) {
                    Some(before) => alerts.extend(
                        group
                            .items
                            .iter()
                            .filter(|item| !before.contains(&item.id))
                            .map(alert),
                    ),
                    None => alerts.extend(group.away.iter().map(alert)),
                }
                let ids = group.items.iter().map(|item| item.id.clone());
                seen.insert(key, ids.collect());
            }
        }
        self.seen = seen;
        alerts
    }
}
