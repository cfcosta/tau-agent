//! tau-mcp without its host half, as on a phone: no servers connect
//! here. The host the interface drives sends the servers it sees.

use super::*;

pub type Host = ();

pub(super) fn agent_plugins(
    _host: &Host,
    _run: &RunCtx,
    _settings: &Settings,
) -> anyhow::Result<Vec<Box<dyn Plugin>>> {
    Ok(Vec::new())
}

pub(super) fn catalog(
    _host: &Host,
    _cx: &HostCx,
    _settings: &Settings,
) -> PluginInfo {
    PluginInfo::default()
}

pub(super) fn data(_host: &Host, _cx: &HostCx) -> Servers {
    Servers::default()
}

pub(super) fn repo_data(
    _host: &Host,
    _repo: &RepoCtx,
    _cx: &HostCx,
) -> Servers {
    Servers::default()
}

pub(super) async fn act(
    _host: &Host,
    _action: Value,
    _cx: &HostCx,
) -> anyhow::Result<Option<Value>> {
    anyhow::bail!("{NAME} has no host half here")
}
