//! tau-luau-plugins: plugins written in Luau, run in codemode's sandbox,
//! as one tau plugin
//! ([ADR 0027](../../../docs/decisions/0027-luau-plugins.md)).
//!
//! A Luau plugin is a folder: `plugin.luau`, which returns
//! `tau.plugin { ... }`, modules under `lib/`, tests under `tests/` and
//! a README. The host reads what it declares ([`Declaration`]) and runs
//! each of its hooks in a fresh codemode VM (`runtime`, feature `host`).
//!
//! - [`Declaration`], [`Uses`], [`ToolSpec`], [`Hooks`]: what a plugin
//!   says it is, as every interface shows it.
//! - `runtime` (feature `host`): reading a folder, loading it, calling
//!   its hooks.

#[cfg(feature = "host")]
pub mod agent;
#[cfg(feature = "host")]
pub mod runtime;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The host plugin's name, for both its halves.
pub const NAME: &str = "tau-luau-plugins";

/// The `tau` module every plugin requires: the declaration, the
/// answers hooks give, the view pieces.
pub const TAU_MODULE: &str = include_str!("tau.luau");

/// What a plugin may reach. The host refuses anything else.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uses {
    /// Tools its tool handlers may call, by name.
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub jev: bool,
    #[serde(default)]
    pub infer: bool,
}

impl Uses {
    /// What `self` reaches that `before` did not, in words: what the
    /// person allows before a version that reaches it activates.
    pub fn grown_from(&self, before: &Uses) -> Vec<String> {
        let mut grown: Vec<String> = self
            .tools
            .iter()
            .filter(|tool| !before.tools.contains(tool))
            .map(|tool| format!("the `{tool}` tool"))
            .collect();
        if self.jev && !before.jev {
            grown.push("Jev".into());
        }
        if self.infer && !before.infer {
            grown.push("`infer`".into());
        }
        grown
    }
}

/// A tool a plugin adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// The JSON Schema of its arguments.
    pub parameters: Value,
    /// It draws its own card.
    #[serde(default)]
    pub card: bool,
}

/// Which hooks a plugin has.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub struct Hooks {
    #[serde(default)]
    pub before_tool: bool,
    #[serde(default)]
    pub before_stop: bool,
    #[serde(default)]
    pub turn_end: bool,
    #[serde(default)]
    pub run_end: bool,
    #[serde(default)]
    pub view: bool,
}

impl Hooks {
    /// The seams `self` has that `before` did not: a hook that can block
    /// a call or keep a run going reaches further.
    pub fn grown_from(&self, before: &Hooks) -> Vec<String> {
        let mut grown = Vec::new();
        if self.before_tool && !before.before_tool {
            grown.push("`before_tool`".into());
        }
        if self.before_stop && !before.before_stop {
            grown.push("`before_stop`".into());
        }
        grown
    }
}

/// What a plugin declares, read without running its hooks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Declaration {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub uses: Uses,
    /// `{ schema, default }`, when it has settings.
    #[serde(default)]
    pub settings: Option<Value>,
    #[serde(default)]
    pub tools: Vec<ToolSpec>,
    #[serde(default)]
    pub hooks: Hooks,
    #[serde(default)]
    pub actions: Vec<String>,
}

impl Declaration {
    /// What `self` reaches that `before` did not: its new tools to call,
    /// Jev, `infer`, and the seams that block or continue. Empty when a
    /// version may activate by itself.
    pub fn grown_from(&self, before: &Declaration) -> Vec<String> {
        let mut grown = self.uses.grown_from(&before.uses);
        grown.extend(self.hooks.grown_from(&before.hooks));
        grown
    }

    /// Its settings' defaults, or an empty object.
    pub fn default_settings(&self) -> Value {
        self.settings
            .as_ref()
            .and_then(|settings| settings.get("default"))
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default()))
    }
}

/// What the host publishes about a run's Luau plugins, as tau-luau-plugins'
/// records: live and stored runs, on the computer and the phone, fold
/// them the same way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Record {
    /// A plugin's state for the run, whole, after a hook changed it.
    State { plugin: String, state: Value },
    /// What its `view` drew from that state.
    View { plugin: String, view: Value },
    /// Lines it wrote with `ctx.log`.
    Log { plugin: String, lines: Vec<String> },
    /// A hook failed: it counted as allowing, or as stopping.
    Error {
        plugin: String,
        hook: String,
        error: String,
    },
    /// `before_tool` let a call through, flagged for review.
    Flag {
        plugin: String,
        call_id: String,
        reason: String,
    },
    /// The plugin is off for the rest of the run, and why.
    Off { plugin: String, why: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declaring(tools: &[&str], jev: bool, before_tool: bool) -> Declaration {
        Declaration {
            name: "p".into(),
            description: String::new(),
            uses: Uses {
                tools: tools.iter().map(|t| t.to_string()).collect(),
                jev,
                infer: false,
            },
            settings: None,
            tools: Vec::new(),
            hooks: Hooks {
                before_tool,
                ..Hooks::default()
            },
            actions: Vec::new(),
        }
    }

    /// A version reaches further when it calls a new tool, asks Jev, or
    /// gains a seam that blocks; dropping any of them is not growth.
    #[test]
    fn growth_is_what_a_version_newly_reaches() {
        let before = declaring(&["bash"], false, false);
        assert!(before.grown_from(&before).is_empty());
        let after = declaring(&["bash", "read"], true, true);
        assert_eq!(
            after.grown_from(&before),
            ["the `read` tool", "Jev", "`before_tool`"]
        );
        assert!(before.grown_from(&after).is_empty());
    }
}
