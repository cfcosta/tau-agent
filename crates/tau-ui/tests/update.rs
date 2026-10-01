//! What the host and an interface send each other reads back the same:
//! the demo's runs, its updates and the workspace's events.

use serde::{Deserialize, Serialize};
use tau_ui::demo;
use tau_ui_remote::{
    update::HostUpdate,
    view::RunUpdate,
    workspace::WorkspaceEvent,
};

fn round_trip<T>(value: &T) -> T
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

#[test]
fn a_snapshot_of_the_demo_round_trips() {
    let mut runs = vec![demo::retry_after()];
    runs.extend(demo::history());
    let snapshot = HostUpdate::Snapshot {
        runs,
        catalog: Box::new(demo::catalog()),
    };
    assert_eq!(round_trip(&snapshot), snapshot);
}

#[test]
fn updates_round_trip() {
    let run = demo::run_id();
    let updates = [
        HostUpdate::alert("Could not start the run", "no model"),
        HostUpdate::PullRequest {
            run: run.clone(),
            pr: Box::new(demo::pull_request()),
        },
        HostUpdate::PullRequestState {
            run: run.clone(),
            state: demo::opened(),
        },
        HostUpdate::Dropped {
            run: run.clone(),
            result: Err("it has children".into()),
        },
        HostUpdate::ResumeFailed(run.clone()),
        HostUpdate::Titled {
            run,
            title: "Fix the retry loop".into(),
        },
    ];
    for update in updates {
        assert_eq!(round_trip(&update), update);
    }
    let events =
        demo::script()
            .into_iter()
            .filter_map(|(_, update)| match update {
                RunUpdate::Event(event) => Some(event),
                _ => None,
            });
    for event in events {
        let update = HostUpdate::Event(event);
        assert_eq!(round_trip(&update), update);
    }
}

#[test]
fn workspace_events_round_trip() {
    let events = [
        WorkspaceEvent::Steer {
            run: demo::run_id(),
            text: "honor retry-after".into(),
        },
        WorkspaceEvent::Cancel {
            run: demo::run_id(),
        },
    ];
    for event in events {
        assert_eq!(round_trip(&event), event);
    }
}
