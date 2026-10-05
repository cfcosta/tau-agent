//! Checks a run's tool calls and final answer against a constitution,
//! asking Jev how likely each applicable rule is broken
//! (`docs/reference/plugins.md`, `tau-constitution`): tau-constitution's
//! host half. Its rules' shapes, its records and its page are
//! `tau-constitution`'s (ADR 0030).
//!
//! - **Before a tool call**, the rules that name one of the call's
//!   arguments (`edit.newText`, `bash.command`) are checked, all in one
//!   request. Jev sees only those arguments, as the model wrote them,
//!   never tool output or file content. A rule at or past its `block`
//!   probability refuses the call, and the reason (the rule, quoted)
//!   goes back to the model so it can fix the call. Between `review`
//!   and `block`, the call runs and is flagged for a person.
//! - **Before the run stops**, the rules on the final answer are checked
//!   the same way. A broken one sends the answer back with the rule, up
//!   to `max_holds` times per run.
//! - Every decision is reported
//!   ([`PluginCtx::report`](tau_agent::plugin::PluginCtx::report)) and
//!   recorded with the run, as a [`Verdict`], so interfaces can show and
//!   review them, now and in history.
//! - When Jev gives no answer, `on_error` decides: `allow` (the default)
//!   lets the call run, or the answer stand; `block` refuses the call,
//!   or sends the answer back while holds are left. Either way the
//!   failure is reported and recorded (`"kind": "error"`).
//! - The rules are read again at every check, from a [`Live`] handle the
//!   host updates when they are edited: an edit applies from the next
//!   tool call, in runs already going too.

mod checks;
pub mod db;
#[cfg(feature = "demo")]
pub mod demo;
mod half;

pub use checks::{ConstitutionPlugin, Live, try_rule};
pub use db::ConstitutionError;
pub use half::{ConstitutionHost, Host};
use tau_constitution::rules;
// The rules and records, from the interface half, by the paths the host
// half has always used.
pub use tau_constitution::{
    Check,
    Constitution,
    Failure,
    NAME,
    OnError,
    Record,
    Rule,
    RuleError,
    Score,
    StoredConstitution,
    StoredRule,
    Target,
    Trial,
    Verdict,
    VerdictKind,
};
