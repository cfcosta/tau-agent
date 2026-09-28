//! The host runs agents and reports their events; the view folds them.

use std::time::Duration;

use tau_agent::{
    agent::Agent,
    event::{RunEvent, StopReason},
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_ui::{
    host::{Access, Host, HostConfig},
    view::{Item, RunStatus},
};
use tokio::sync::mpsc::UnboundedReceiver;

fn host(llm: ScriptedModel) -> (Host, UnboundedReceiver<RunEvent>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::memory()).unwrap();
    let agent = Agent::new(llm).name("coder");
    let config = HostConfig {
        access: Access::ApiKey,
        model: "gpt-5.5".into(),
        root: std::env::temp_dir(),
        store: std::env::temp_dir().join("unused.db"),
    };
    Host::with_agent(runtime, agent, store, config)
}

/// Receives until `RunEnd`, with a timeout so a hang fails the test.
fn until_end(events: &mut UnboundedReceiver<RunEvent>) -> Vec<RunEvent> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut seen = Vec::new();
    while std::time::Instant::now() < deadline {
        match events.try_recv() {
            Ok(event) => {
                let end = matches!(event, RunEvent::RunEnd { .. });
                seen.push(event);
                if end {
                    return seen;
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
    panic!("no RunEnd within 10 s: {seen:?}");
}

#[test]
fn a_run_streams_into_its_view() {
    let llm = ScriptedModel::new().turn(|t| t.text("Hello from tau"));
    let (host, mut events) = host(llm);
    let mut view = host.start("Say hello, please");
    assert_eq!(view.title, "say-hello-please");
    assert!(
        matches!(view.items.first(), Some(Item::User(text)) if text == "Say hello, please")
    );
    for event in until_end(&mut events) {
        view.apply(&event);
    }
    assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
    assert_eq!(view.last_text(), Some("Hello from tau"));
    // The run leaves the host's table once its outcome is in.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while host.is_running(&view.id) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!host.is_running(&view.id));
}

#[test]
fn a_run_can_be_cancelled_from_the_ui() {
    // A response that takes 30 s to start: cancelled long before.
    let llm = ScriptedModel::new()
        .turn(|t| t.delay(Duration::from_secs(30)).text("too late"));
    let (host, mut events) = host(llm);
    let view = host.start("wait");
    std::thread::sleep(Duration::from_millis(100));
    assert!(host.is_running(&view.id));
    host.cancel(&view.id);
    let ends: Vec<StopReason> = until_end(&mut events)
        .into_iter()
        .filter_map(|event| match event {
            RunEvent::RunEnd { stop, .. } => Some(stop),
            _ => None,
        })
        .collect();
    assert_eq!(ends, [StopReason::Cancelled]);
}
