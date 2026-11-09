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
        n if n == tau_ui_plugin::loop_::LOOP.name() => {
            "the loop streams the model's replies and holds a turn's tool \
             calls running, which a hook cannot"
        }
        n if n == tau_ui_plugin::provider::PROVIDER.name() => {
            "a provider streams the model's replies, which a hook cannot"
        }
        n if n == tau_ui_plugin::title::TITLE.name() => {
            return Some(Luau::Hook("title"));
        }
        n if n == tau_ui_plugin::compaction::COMPACTION.name() => {
            return Some(Luau::Hook("compaction"));
        }
        n if n == tau_ui_plugin::notify::NOTIFY.name() => {
            return Some(Luau::Hook("notify"));
        }
        n if n == tau_ui_plugin::forge::FORGE.name() => {
            return Some(Luau::Hook("forge"));
        }
        _ => return None,
    };
    debug_assert!(slots::ALL.contains(&name));
    Some(Luau::Excluded(excluded))
}

/// How a Luau plugin draws at the point `name`; `None` for a point this
/// table does not know.
pub fn point(name: &str) -> Option<Luau> {
    use points::*;
    let hooks = [
        (STATUS.name, "status"),
        (TRANSCRIPT.name, "note"),
        (CARD.name, "card"),
        (CARD_BADGE.name, "tau.flag(why)"),
        (RUN_BANNER.name, "banner"),
        (INSPECTOR.name, "inspector"),
        (COMPOSER_BAND.name, "band"),
        (RUN_ROW.name, "row"),
        (ASKS.name, "asks"),
        (SIDEBAR.name, "nav"),
        (PLAN.name, "plan"),
        (PLAN_STEPS.name, "plan_steps"),
        (PICKER_AUTO.name, "picker_auto"),
        (USER_MESSAGE.name, "messages"),
        (CARD_BODY.name, "calls"),
        (COMPOSER.name, "composer"),
        (PANE.name, "pane"),
    ];
    if let Some((_, hook)) = hooks.iter().find(|(point, _)| *point == name) {
        return Some(Luau::Hook(hook));
    }
    let outside = "drawn outside any run, where a Luau plugin has no view; \
                   its place there is its sidebar entry (`nav`)";
    let excluded = [
        (CARD_STAT.name, "a tool's `card` draws its whole card"),
        (
            REPO_KIND.name,
            "tau-luau-plugins' own: the plugins repository",
        ),
        (
            KIND_ROW_END.name,
            "a repository kind's, which a Rust plugin keeps",
        ),
        (
            KIND_ROW_LINE.name,
            "a repository kind's, which a Rust plugin keeps",
        ),
        (
            CONTEXT_TRIGGER.name,
            "a typed answer the kernel reads, not a tree",
        ),
        (
            SETUP.name,
            "onboarding runs before the person's plugins load",
        ),
        (EVERYWHERE.name, outside),
        (SEARCH.name, outside),
        (SIDEBAR_REPO.name, outside),
        (REPO_MENU.name, outside),
        (REPO_ACTIONS.name, outside),
        (ACTIVITY.name, outside),
        (MODELS.name, outside),
        (WAITING.name, outside),
        (REWRITE.name, NOT_YET),
        (CONTEXT.name, NOT_YET),
        (CHANGES.name, NOT_YET),
    ];
    excluded
        .iter()
        .find(|(point, _)| *point == name)
        .map(|(_, why)| Luau::Excluded(why))
}

/// How a Luau plugin is given the service `name`; `None` for a service
/// this table does not know.
pub fn service(name: &str) -> Option<Luau> {
    let luau = match name {
        tau_jev::SERVICE => Luau::Hook("ctx.jev"),
        names::WORKSPACE_DIR => Luau::Hook("ctx.files"),
        names::IDENTITY => Luau::Excluded(
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
        names::MODEL => Luau::Hook("infer"),
        names::CONVERSATIONS => Luau::Hook("tau.start_chat"),
        // tau-skills' directory.
        "tau.skills-dir" => {
            Luau::Excluded("a plugin's own, given to the plugin that reads it")
        }
        _ => return None,
    };
    Some(luau)
}
