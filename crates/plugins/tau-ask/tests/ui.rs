//! tau-ask's UI half: who gets the tool. Its design values are checked
//! with every plugin's, in tau-ui-kit's tests.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

/// Every run gets `ask`, so their tools match and share the prompt
/// cache; a sub-agent's, which nobody watches, refuses every call.
#[cfg(feature = "host")]
#[test]
fn every_run_gets_ask_and_a_sub_agent_s_refuses() {
    use tau_agent::tool::ToolCtx;
    use tau_ui_plugin::{HostHalf as _, RunKind, testing::run_ctx};
    let waiting = tau_ask::host::Waiting::default();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    for kind in [RunKind::Main, RunKind::Chat, RunKind::SubAgent] {
        let plugins = tau_testing::block_on(tau_ask::AskHost.agent_plugins(
            &waiting,
            &run_ctx(kind),
            &(),
        ))
        .unwrap();
        let tools: Vec<_> =
            plugins.iter().flat_map(|plugin| plugin.tools()).collect();
        let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
        assert_eq!(names, [tau_ask::TOOL], "{kind:?}");
        let args = serde_json::json!({ "questions": [] });
        let answer = runtime.block_on(tools[0].call(args, ToolCtx::detached()));
        let refused = matches!(
            &answer,
            Err(error) if error.to_string() == tau_ask::host::NO_ONE_TO_ASK
        );
        assert_eq!(refused, kind == RunKind::SubAgent, "{kind:?}: {answer:?}");
    }
}
