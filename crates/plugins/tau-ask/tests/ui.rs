//! tau-ask's UI half: who gets the tool. Its design values are checked
//! with every plugin's, in tau-ui-kit's tests.

/// A run a person watches gets `ask`; a sub-agent, which nobody
/// watches, does not.
#[cfg(feature = "host")]
#[test]
fn every_run_but_a_sub_agent_gets_ask() {
    use tau_ui_plugin::{RunKind, UiPlugin, testing::run_ctx};
    let waiting = tau_ask::host::Waiting::default();
    for kind in [RunKind::Main, RunKind::Chat, RunKind::SubAgent] {
        let plugins = tau_ask::AskUi
            .agent_plugins(&waiting, &run_ctx(kind), &())
            .unwrap();
        let tools: Vec<String> = plugins
            .iter()
            .flat_map(|plugin| plugin.tools())
            .map(|tool| tool.name().to_owned())
            .collect();
        let expected: &[&str] = if kind == RunKind::SubAgent {
            &[]
        } else {
            &[tau_ask::TOOL]
        };
        assert_eq!(tools, expected, "{kind:?}");
    }
}
