//! tau-jev's host half: it keeps the TypeSafe key, and gives the plugins
//! that ask Jev a metered client while there is one (ADR 0045).

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use tau_agent::plugin::Plugin;
use tau_ui_plugin::{
    Group,
    HostCx,
    HostHalf,
    PluginHost,
    PluginInfo,
    Provide,
    RunCtx,
};

use crate::{
    DEFAULT_MODEL,
    Jev,
    TypeSafe,
    metered::{Meter, Metered},
    ui::{Act, Data, JevUi, Stats},
};

/// The host half for tau's registry.
#[derive(Debug, Clone, Copy, Default)]
pub struct JevHost;

/// A Jev to ask in place of TypeSafe's, as if a key were saved: a host
/// value (`tau_ui_plugin::Services::with`) for tests and the demo.
#[derive(Clone)]
pub struct Upstream(pub Arc<dyn Jev>);

/// What the host half keeps: the key, read once and kept as it changes
/// (Jev is asked for often), and what every request did this session.
pub struct Host {
    path: Option<PathBuf>,
    key: Mutex<Option<String>>,
    meter: Arc<Mutex<Meter>>,
}

impl Host {
    /// The Jev to give, when there is one: the host's stand-in, else
    /// TypeSafe's with the saved key.
    fn jev(&self, upstream: Option<&Upstream>) -> Option<Arc<dyn Jev>> {
        let inner =
            upstream.map(|upstream| upstream.0.clone()).or_else(|| {
                let key = self.key.lock().expect("not poisoned").clone()?;
                Some(Arc::new(TypeSafe::new(key)) as Arc<dyn Jev>)
            })?;
        Some(Arc::new(Metered::new(inner, self.meter.clone())))
    }

    fn keyed(&self) -> bool {
        self.key.lock().expect("not poisoned").is_some()
    }
}

impl PluginHost for Host {
    async fn new(cx: &HostCx) -> anyhow::Result<Self> {
        let path = cx.config_dir().map(|dir| dir.join("typesafe-key"));
        let key = match &path {
            Some(path) => tokio::fs::read_to_string(path)
                .await
                .ok()
                .map(|key| key.trim().to_owned())
                .filter(|key| !key.is_empty()),
            None => None,
        };
        Ok(Self {
            path,
            key: Mutex::new(key),
            meter: Arc::default(),
        })
    }
}

impl HostHalf for JevHost {
    type Plugin = JevUi;
    type Host = Host;

    async fn agent_plugins(
        &self,
        _: &Host,
        _: &RunCtx,
        _: &(),
    ) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
        Ok(Vec::new())
    }

    async fn catalog(&self, host: &Host, cx: &HostCx, _: &()) -> PluginInfo {
        let given = cx.services.get::<Upstream>().is_some() || host.keyed();
        PluginInfo {
            description: if given {
                "Gives the plugins that ask it TypeSafe's Jev"
            } else {
                "Gives the plugins that ask it TypeSafe's Jev: needs a key \
                 (Models)"
            }
            .into(),
            group: Group::App,
            ..Default::default()
        }
    }

    async fn data(&self, host: &Host, cx: &HostCx) -> Data {
        let keyed = cx.services.get::<Upstream>().is_some() || host.keyed();
        let meter = host.meter.lock().expect("not poisoned").clone();
        Data {
            keyed,
            stats: keyed.then(|| Stats {
                model: meter
                    .model
                    .clone()
                    .unwrap_or_else(|| DEFAULT_MODEL.into()),
                requests: meter.requests,
                input_tokens: meter.input_tokens,
                spent: meter.spent,
                latency_p50_ms: meter.latency_p50_ms(),
                failed: meter.failed,
            }),
        }
    }

    fn provide(&self, host: &Host, services: &mut Provide<'_>) {
        if let Some(jev) = host.jev(services.get::<Upstream>()) {
            services.offer(crate::SERVICE, jev);
        }
    }

    async fn act(
        &self,
        host: &Host,
        action: serde_json::Value,
        cx: &HostCx,
    ) -> anyhow::Result<Option<serde_json::Value>> {
        let Act::Key { key } = serde_json::from_value(action)?;
        let key = key
            .map(|key| key.trim().to_owned())
            .filter(|key| !key.is_empty());
        if let Some(path) = &host.path {
            match &key {
                Some(key) => {
                    tau_ai::files::write_private_async(path, key.as_bytes())
                        .await?
                }
                None => match tokio::fs::remove_file(path).await {
                    Err(error)
                        if error.kind() != std::io::ErrorKind::NotFound =>
                    {
                        return Err(error.into());
                    }
                    _ => {}
                },
            }
        }
        *host.key.lock().expect("not poisoned") = key;
        cx.refresh();
        Ok(None)
    }
}
