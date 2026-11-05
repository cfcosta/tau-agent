//! What a Luau plugin reaches of the kernel (ADR 0045, step 10): every
//! seam, slot, point and service is a hook or field a Luau plugin writes,
//! or is left out, with why. `tests/parity.rs` checks the table against
//! the kernel's own lists, so a seam, slot, point or service added
//! without a word on Luau fails.

use tau_ui_plugin::{Seam, points, services::names, slots};

/// How a Luau plugin reaches one part of the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Luau {
    /// Through this hook or field, as the skill names it.
    Hook(&'static str),
    /// Not at all, for this reason.
    Excluded(&'static str),
}

/// Not reached yet; a later part of step 10 adds it.
const NOT_YET: &str = "not yet";

/// How a Luau plugin steps in at `seam`.
pub fn seam(seam: Seam) -> Luau {
    match seam {
        Seam::Tools => Luau::Hook("tools"),
        Seam::Start => Luau::Hook("start"),
        Seam::BeforeTool => Luau::Hook("before_tool"),
        Seam::BeforeRequest => Luau::Hook("before_request"),
        Seam::AfterTool => Luau::Hook("after_tool_result"),
        Seam::Rewrite | Seam::StartIdle => Luau::Hook("rewrite_context"),
        Seam::Rewritten => Luau::Hook("rewritten"),
        Seam::OnEvent => Luau::Hook("on_event"),
        Seam::BeforeStop => Luau::Hook("before_stop"),
        Seam::Finish => Luau::Hook("run_end"),
    }
}

/// How a Luau plugin fills the slot `name`; `None` for a slot this table
/// does not know.
pub fn slot(name: &str) -> Option<Luau> {
    let excluded = match name {
        n if n == tau_ui_plugin::store::STORE.name() => {
            "the store opens before any plugin loads, Luau's among them"
        }
        n if n == tau_ui_plugin::vcs::VCS.name() => {
            "landing, lanes and the sweep hold workspaces open for longer \
             than a hook may run"
        }
        n if n == tau_ui_plugin::provider::PROVIDER.name() => {
            "a provider streams the model's replies, which a hook cannot"
        }
        n if n == tau_ui_plugin::forge::FORGE.name()
            || n == tau_ui_plugin::compaction::COMPACTION.name()
            || n == tau_ui_plugin::title::TITLE.name()
            || n == tau_ui_plugin::notify::NOTIFY.name() =>
        {
            NOT_YET
        }
        _ => return None,
    };
    debug_assert!(slots::ALL.contains(&name));
    Some(Luau::Excluded(excluded))
}

/// How a Luau plugin draws at the point `name`; `None` for a point this
/// table does not know.
pub fn point(name: &str) -> Option<Luau> {
    let luau = match name {
        n if n == points::STATUS.name => Luau::Hook("status"),
        n if n == points::TRANSCRIPT.name => Luau::Hook("note"),
        n if n == points::CARD.name => Luau::Hook("card"),
        n if n == points::CARD_BADGE.name => Luau::Hook("tau.flag(why)"),
        n if points::ALL.contains(&n) => Luau::Excluded(NOT_YET),
        _ => return None,
    };
    Some(luau)
}

/// How a Luau plugin is given the service `name`; `None` for a service
/// this table does not know.
pub fn service(name: &str) -> Option<Luau> {
    let luau = match name {
        tau_jev::SERVICE => Luau::Hook("ctx.jev"),
        names::WORKSPACE_DIR | names::IDENTITY => Luau::Excluded(
            "the host's own, to check and land the plugins repository",
        ),
        names::TURN_HOOKS
        | names::RUN_COMMANDS
        | names::RUN_WAKE
        | names::AGENT_REPO
        | names::REPO_LAUNCHER
        | names::OUTSIDE_TOOLS
        | names::OUTSIDE_RESOURCES
        | names::UNATTENDED => Luau::Excluded(
            "what one run or the host keeps for its own tools, which a \
             plugin reaches as those tools",
        ),
        names::APP => Luau::Excluded(
            "carrying the window to phones is the phones plugin's",
        ),
        names::CONVERSATIONS | names::MODEL => Luau::Excluded(NOT_YET),
        // tau-memory's search and tau-skills' directory.
        "tau.memory-search" | "tau.skills-dir" => {
            Luau::Excluded("a plugin's own, given to the plugin that reads it")
        }
        _ => return None,
    };
    Some(luau)
}
