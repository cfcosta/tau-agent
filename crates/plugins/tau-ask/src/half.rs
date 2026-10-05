//! tau-ask's host half (ADR 0030).

use serde_json::Value;
use tau_agent::plugin::Plugin;
use tau_ui_plugin::{HostCx, HostHalf, PluginInfo, RunCtx, Seam};

use crate::{
    AskUi,
    ui::{Act, Refused},
};

/// The host half's state: the calls waiting, shared by every run.
type Host = crate::host::Waiting;

/// tau-ask on the host.
#[derive(Debug, Clone, Copy, Default)]
pub struct AskHost;

impl HostHalf for AskHost {
    type Plugin = AskUi;
    type Host = Host;

    /// The `ask` tool. A sub-agent has no one to ask: its `ask` refuses,
    /// and is there so its tools match its caller's.
    async fn agent_plugins(
        &self,
        host: &Host,
        run: &RunCtx,
        _settings: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        let plugin = crate::host::AskPlugin::new(host.clone());
        Ok(vec![Box::new(
            if run.kind == tau_ui_plugin::RunKind::SubAgent {
                plugin.refusing()
            } else {
                plugin
            },
        )])
    }

    async fn catalog(
        &self,
        _host: &Host,
        _cx: &HostCx,
        _settings: &(),
    ) -> PluginInfo {
        PluginInfo {
            description:
                "Lets the agent ask you questions and waits for your answers"
                    .into(),
            seams: vec![Seam::Start, Seam::Tools],
            page: None,
            ..Default::default()
        }
    }

    async fn act(
        &self,
        host: &Host,
        action: Value,
        _cx: &HostCx,
    ) -> anyhow::Result<Option<Value>> {
        let act: Act = serde_json::from_value(action)?;
        // Refused, the panel says why and takes answers again.
        Ok(host
            .answer(&act.run, &act.call, act.reply)
            .err()
            .map(|error| {
                serde_json::to_value(Refused {
                    run: act.run,
                    call: act.call,
                    message: format!("{error:#}"),
                })
                .expect("a refusal serializes")
            }))
    }
}
