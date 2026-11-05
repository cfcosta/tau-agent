//! tau-phones' host half: the server phones connect to, on the window
//! the host gives as [`tau_ui_plugin::app::App`].

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{Group, HostCx, HostHalf, PluginHost, PluginInfo, RunCtx};

use crate::{Act, PhonesData, PhonesUi, bridge::Phones};

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhonesHost;

/// What the host half keeps: phones, when the host has a window to
/// mirror and a directory to keep them in.
pub struct Host {
    phones: Option<Phones>,
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let (Some(window), Some(dir)) = (cx.app(), cx.config_dir()) else {
            return Ok(Self { phones: None });
        };
        let refresh = cx.clone();
        let phones = Phones::open(
            dir.join("phones"),
            window,
            std::sync::Arc::new(move || refresh.refresh()),
        )
        .await;
        Ok(Self {
            phones: Some(phones),
        })
    }
}

impl HostHalf for PhonesHost {
    type Plugin = PhonesUi;
    type Host = Host;

    async fn agent_plugins(
        &self,
        _: &Host,
        _: &RunCtx,
        _: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(&self, host: &Host, _: &HostCx, _: &()) -> PluginInfo {
        let allowed = host.phones.as_ref().is_some_and(|p| p.data().allowed);
        PluginInfo {
            description: if allowed {
                "Phones reach tau on this computer"
            } else {
                "Lets phones reach tau on this computer: off"
            }
            .into(),
            group: Group::App,
            ..Default::default()
        }
    }

    async fn data(&self, host: &Host, _: &HostCx) -> PhonesData {
        host.phones.as_ref().map(Phones::data).unwrap_or_default()
    }

    /// Listens for phones as the host starts, if they were allowed.
    async fn resume(&self, host: &Host, _: &HostCx) {
        if let Some(phones) = &host.phones {
            phones.resume();
        }
    }

    async fn act(
        &self,
        host: &Host,
        action: serde_json::Value,
        _: &HostCx,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let act: Act = serde_json::from_value(action)?;
        if let Some(phones) = &host.phones {
            phones.act(act).await;
        }
        Ok(None)
    }
}
