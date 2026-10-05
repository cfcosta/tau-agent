//! tau on the computer: tau-ui's host and interface, with every
//! plugin's host half. The Luau ones (tau-codemode, tau-luau-plugins)
//! are linked here and nowhere above, so tau-ui builds while Luau's C++
//! does (ADR 0030).

use std::sync::Once;

use tau_ui_plugin::Registry;
use tau_ui_remote::plugins;

/// Every plugin tau-ui-remote lists, each with its host half.
pub fn plugins() -> Registry {
    tau_ui::hosted::halves(plugins::plugins())
        .host(tau_luau_plugins_host::LuauPluginsHost)
        .host(tau_codemode_host::CodemodeHost)
}

/// Installs [`plugins`] as this process's, once: before anything starts
/// a host.
pub fn install() {
    static INSTALLED: Once = Once::new();
    INSTALLED.call_once(|| plugins::install(plugins()));
}
