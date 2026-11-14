//! heiko on the computer: heiko-ui's host and interface, with every
//! plugin's host half. The Luau ones (heiko-codemode, heiko-luau-plugins,
//! heiko-agents) are linked here and nowhere above, so heiko-ui builds while
//! Luau's C++ does (ADR 0030).

use std::sync::Once;

use heiko_ui_plugin::Registry;
use heiko_ui_remote::plugins;

/// Every plugin heiko-plugins lists, each with its host half.
pub fn plugins() -> Registry {
    heiko_ui::hosted::halves(heiko_plugins::plugins())
        .host(heiko_luau_plugins_host::LuauPluginsHost)
        .host(heiko_codemode_host::CodemodeHost)
        .host(heiko_agents_host::AgentsHost)
}

/// Loads the Luau plugins heiko comes with: a failure means no agents, with
/// its reason, as they provide what a run cannot go without (ADR 0051).
pub fn load_built_in_plugins() -> anyhow::Result<()> {
    heiko_luau_plugins_host::built_in::load_at_start()
}

/// Installs [`plugins`] as this process's, once: before anything starts
/// a host.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| plugins::install(plugins()));
}
