//! What a phone sends the computer (decision 0013), and which of it a
//! phone may ask. The computer's side of phones is the heiko-phones
//! plugin's.

use serde::{Deserialize, Serialize};

use crate::workspace::WorkspaceEvent;

/// What a phone sends up, inside the wire's frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PhoneUp {
    /// Something the person did on the phone, for the host.
    Event(WorkspaceEvent),
    /// The phone's name, as the computer lists it.
    Name(String),
}

impl WorkspaceEvent {
    /// Whether a phone may ask for this. Pairing and paths on the
    /// computer stay with the computer. Every event is named here, so a
    /// new one is decided before it builds.
    pub fn from_phone(&self) -> bool {
        match self {
            Self::Pair(_) | Self::OpenRepos(_) => false,
            Self::Ui(_)
            | Self::NewRun { .. }
            | Self::HideRepo { .. }
            | Self::UpdateRepo { .. }
            | Self::PluginAct { .. }
            | Self::PluginCall { .. }
            | Self::AskViews(_)
            | Self::DropViews(_)
            | Self::PluginFetch { .. }
            | Self::PluginSettings { .. }
            | Self::PluginRecord { .. }
            | Self::CloseRun { .. }
            | Self::Say { .. }
            | Self::Picture { .. }
            | Self::Cancel { .. }
            | Self::Fork { .. }
            | Self::SetDefaultModel { .. }
            | Self::HideModel { .. }
            | Self::KeepBranch { .. }
            | Self::PreviewLanding { .. }
            | Self::Land { .. }
            | Self::DropChild { .. }
            | Self::Unqueue { .. }
            | Self::ResolveAgain { .. }
            | Self::DismissConflicts { .. }
            | Self::Query { .. }
            | Self::ResumeCutOff { .. }
            | Self::CloneRepos { .. }
            | Self::SetupFinished { .. }
            | Self::PreparePullRequest { .. }
            | Self::CompareCode { .. }
            | Self::Push { .. }
            | Self::CreatePullRequest { .. } => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which variant `event` is, numbered in declaration order. The match
    /// has no wildcard, so a new event must be added here, and so to the
    /// table below.
    fn variant(event: &WorkspaceEvent) -> usize {
        use WorkspaceEvent as E;
        match event {
            E::Ui(_) => 0,
            E::NewRun { .. } => 1,
            E::OpenRepos(_) => 2,
            E::HideRepo { .. } => 3,
            E::UpdateRepo { .. } => 4,
            E::PluginAct { .. } => 5,
            E::PluginCall { .. } => 6,
            E::AskViews(_) => 7,
            E::DropViews(_) => 8,
            E::PluginFetch { .. } => 9,
            E::PluginSettings { .. } => 10,
            E::PluginRecord { .. } => 11,
            E::CloseRun { .. } => 12,
            E::Say { .. } => 13,
            E::Picture { .. } => 14,
            E::Cancel { .. } => 15,
            E::Fork { .. } => 16,
            E::SetDefaultModel { .. } => 17,
            E::HideModel { .. } => 18,
            E::KeepBranch { .. } => 19,
            E::PreviewLanding { .. } => 20,
            E::Land { .. } => 21,
            E::DropChild { .. } => 22,
            E::Unqueue { .. } => 23,
            E::ResolveAgain { .. } => 24,
            E::DismissConflicts { .. } => 25,
            E::Query { .. } => 26,
            E::Pair(_) => 27,
            E::ResumeCutOff { .. } => 28,
            E::CloneRepos { .. } => 29,
            E::SetupFinished { .. } => 30,
            E::PreparePullRequest { .. } => 31,
            E::CompareCode { .. } => 32,
            E::Push { .. } => 33,
            E::CreatePullRequest { .. } => 34,
        }
    }

    /// One event of every variant, with whether a phone may ask for it.
    fn table() -> Vec<(WorkspaceEvent, bool)> {
        use WorkspaceEvent as E;
        use serde_json::json;
        let run = || heiko_agent::tool::RunId("r".into());
        vec![
            (
                E::Ui(heiko_ui_plugin::UiEvent {
                    plugin: "todo".into(),
                    key: "open".into(),
                    value: Some(json!(true)),
                }),
                true,
            ),
            (
                E::NewRun {
                    prompt: "go".into(),
                    model: Default::default(),
                    repo: "heiko".into(),
                    images: Vec::new(),
                },
                true,
            ),
            (E::OpenRepos(vec!["heiko".into()]), false),
            (
                E::HideRepo {
                    repo: "heiko".into(),
                },
                true,
            ),
            (
                E::UpdateRepo {
                    repo: "heiko".into(),
                },
                true,
            ),
            (
                E::PluginAct {
                    plugin: "todo".into(),
                    action: json!({}),
                    asker: Some(1),
                },
                true,
            ),
            (
                E::PluginCall {
                    id: 1,
                    plugin: "todo".into(),
                    origin: Default::default(),
                    layout: Default::default(),
                    call: crate::plugins::PluginCall::Key {
                        key: "j".into(),
                        repo: None,
                    },
                },
                true,
            ),
            (E::AskViews(Vec::new()), true),
            (E::DropViews(Vec::new()), true),
            (
                E::PluginFetch {
                    plugin: "todo".into(),
                    key: "k".into(),
                    action: json!({}),
                },
                true,
            ),
            (
                E::PluginSettings {
                    plugin: "todo".into(),
                    repo: None,
                    settings: Some(json!({})),
                },
                true,
            ),
            (
                E::PluginRecord {
                    run: run(),
                    plugin: "todo".into(),
                    body: json!({}),
                },
                true,
            ),
            (E::CloseRun { run: run() }, true),
            (
                E::Say {
                    run: run(),
                    text: "go".into(),
                    model: Default::default(),
                    images: Vec::new(),
                },
                true,
            ),
            (
                E::Picture {
                    run: run(),
                    picture: crate::view::Picture {
                        key: "k".into(),
                        mime: "image/png".into(),
                    },
                },
                true,
            ),
            (E::Cancel { run: run() }, true),
            (
                E::Fork {
                    run: run(),
                    turn: Some(1),
                    prompt: "again".into(),
                    model: Default::default(),
                    images: Vec::new(),
                },
                true,
            ),
            (
                E::SetDefaultModel {
                    agent: "heiko".into(),
                    choice: Default::default(),
                },
                true,
            ),
            (
                E::HideModel {
                    id: "m".into(),
                    hidden: true,
                },
                true,
            ),
            (E::KeepBranch { run: run() }, true),
            (E::PreviewLanding { run: run() }, true),
            (E::Land { run: run() }, true),
            (E::DropChild { run: run() }, true),
            (E::Unqueue { run: run() }, true),
            (E::ResolveAgain { main: run() }, true),
            (E::DismissConflicts { main: run() }, true),
            (
                E::Query {
                    sql: "select 1".into(),
                },
                true,
            ),
            (E::Pair(crate::pairing::PairRequest::Scan), false),
            (E::ResumeCutOff { run: run() }, true),
            (
                E::CloneRepos {
                    repos: vec!["owner/name".into()],
                },
                true,
            ),
            (E::SetupFinished { repos: Vec::new() }, true),
            (E::PreparePullRequest { run: run() }, true),
            (
                E::CompareCode {
                    main: run(),
                    fork: heiko_agent::tool::RunId("f".into()),
                },
                true,
            ),
            (
                E::Push {
                    repo: "heiko".into(),
                    fetch: true,
                },
                true,
            ),
            (
                E::CreatePullRequest {
                    run: run(),
                    title: "t".into(),
                    body: "b".into(),
                    draft: false,
                    keep_pushing: true,
                    reviewers: Vec::new(),
                },
                true,
            ),
        ]
    }

    /// Pairing and paths on the computer stay with the computer; what a
    /// person does with chats, repositories and plugins a phone may ask
    /// for. The table holds one event of every variant, each once.
    #[test]
    fn a_phone_asks_for_what_it_may_and_nothing_the_computer_keeps() {
        let table = table();
        let mut seen: Vec<usize> =
            table.iter().map(|(event, _)| variant(event)).collect();
        seen.sort_unstable();
        let every: Vec<usize> = (0..=34).collect();
        assert_eq!(seen, every, "the table has each variant once");
        for (event, asked) in &table {
            assert_eq!(event.from_phone(), *asked, "{event:?}");
        }
    }
}
