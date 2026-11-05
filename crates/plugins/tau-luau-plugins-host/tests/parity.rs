//! The seam table (ADR 0045, step 10): every seam, slot, point and
//! service the kernel has is one a Luau plugin reaches, through a hook
//! the skill teaches, or one left out, with why.

use std::collections::BTreeSet;

use tau_luau_plugins_host::parity::{self, Luau};
use tau_ui_plugin::{Seam, UiPlugin, points, services::names, slots};

const SKILL: &str = include_str!("../skill/SKILL.md");

/// Each is reached by a hook the skill names, or left out with a reason.
fn holds(what: &str, luau: Luau) {
    match luau {
        Luau::Hook(field) => assert!(
            SKILL.contains(&format!("`{field}`")),
            "{what}: the skill never names `{field}`"
        ),
        Luau::Excluded(why) => {
            assert!(!why.trim().is_empty(), "{what}: left out with no reason")
        }
    }
}

#[test]
fn every_seam_is_a_hook_the_plugin_declares_or_left_out() {
    let declared = tau_luau_plugins::ui::LuauPluginsUi.declaration().reach;
    for seam in Seam::ALL {
        let luau = parity::seam(seam);
        holds(seam.label(), luau);
        if let Luau::Hook(_) = luau {
            assert!(
                declared.seams.contains(&seam),
                "{}: Luau has it, tau-luau-plugins never declares it",
                seam.label()
            );
        }
    }
}

#[test]
fn every_slot_is_filled_from_luau_or_left_out() {
    let fills = tau_luau_plugins::ui::LuauPluginsUi
        .declaration()
        .reach
        .fills;
    for slot in slots::ALL {
        let luau = parity::slot(slot)
            .unwrap_or_else(|| panic!("{slot}: not in Luau's table"));
        holds(slot, luau);
        if let Luau::Hook(_) = luau {
            assert!(fills.iter().any(|f| f == slot), "{slot}: not declared");
        }
    }
}

#[test]
fn every_point_is_drawn_from_luau_or_left_out() {
    for point in points::ALL {
        let luau = parity::point(point)
            .unwrap_or_else(|| panic!("{point}: not in Luau's table"));
        holds(point, luau);
    }
}

/// The host's services and every one a plugin provides or is given.
#[test]
fn every_service_is_given_to_luau_or_left_out() {
    let registry = tau_plugins::plugins();
    let mut services: BTreeSet<String> =
        names::ALL.iter().map(|name| (*name).to_owned()).collect();
    for plugin in registry.plugins() {
        let reach = &plugin.declaration().reach;
        services.extend(reach.services.iter().cloned());
        services.extend(reach.provides.iter().cloned());
    }
    for service in &services {
        let luau = parity::service(service)
            .unwrap_or_else(|| panic!("{service}: not in Luau's table"));
        holds(service, luau);
    }
}
