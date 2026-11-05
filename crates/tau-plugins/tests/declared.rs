//! What tau's plugins declare of themselves holds together (ADR 0045):
//! the plugins they come after or before are tau's, and the registry
//! keeps those orders.

use std::collections::BTreeSet;

#[test]
fn every_plugin_a_declaration_orders_against_is_one_of_taus() {
    let registry = tau_plugins::plugins();
    let names: BTreeSet<&str> =
        registry.plugins().map(|plugin| plugin.name()).collect();
    for plugin in registry.plugins() {
        let declaration = plugin.declaration();
        for other in declaration.after.iter().chain(&declaration.before) {
            assert!(
                names.contains(other),
                "{} orders itself against {other}, which tau does not have",
                plugin.name()
            );
        }
    }
}

#[test]
fn the_registry_keeps_every_declared_order() {
    let registry = tau_plugins::plugins();
    let order: Vec<&str> =
        registry.plugins().map(|plugin| plugin.name()).collect();
    let place = |name: &str| {
        order
            .iter()
            .position(|was| *was == name)
            .unwrap_or_else(|| panic!("{name} is registered"))
    };
    for plugin in registry.plugins() {
        let declaration = plugin.declaration();
        for first in &declaration.after {
            assert!(place(first) < place(plugin.name()), "{order:?}");
        }
        for then in &declaration.before {
            assert!(place(plugin.name()) < place(then), "{order:?}");
        }
    }
}

#[test]
fn the_kernels_slots_are_ones_a_plugin_declares_it_fills() {
    let registry = tau_plugins::plugins();
    let filled: BTreeSet<&str> = registry
        .plugins()
        .flat_map(|plugin| plugin.declaration().reach.fills.iter())
        .map(String::as_str)
        .collect();
    for slot in [
        tau_ui_plugin::vcs::VCS.name(),
        tau_ui_plugin::forge::FORGE.name(),
        tau_ui_plugin::notify::NOTIFY.name(),
        tau_ui_plugin::title::TITLE.name(),
    ] {
        assert!(filled.contains(slot), "nobody declares it fills {slot}");
    }
}
