//! What a codemode call reports while its script runs.
//!
//! A script's tool calls go through the loop, so they make run events
//! of their own, which tau-ui folds into the card's
//! `CallData::nested`. Jev requests do not: the engine asks Jev itself.
//! So each Jev request is reported as the codemode call's own
//! `ToolUpdate`, whose details are a [`JevUpdate`]: its row as it
//! stands, once when it starts and once when it ends, and how many tool
//! calls the script had started before it, which places it among them
//! as the result's `details.calls` does.
//!
//! The card folds them by row id, the latest standing, so the order in
//! which updates and nested events arrive does not matter. Once the
//! call ends the card draws from `details.calls` instead.

use serde_json::{Value, json};

use crate::result::CallRow;

/// One Jev request's row, as a `ToolUpdate`'s details carry it.
#[derive(Debug, Clone, PartialEq)]
pub struct JevUpdate {
    /// The row as `details.calls` would list it now: `running` until
    /// it ends.
    pub row: Value,
    /// The tool calls the script had started before this request.
    pub after: usize,
}

impl JevUpdate {
    pub fn new(row: &CallRow, after: usize) -> Self {
        Self {
            row: row.to_json(),
            after,
        }
    }

    /// `{ "jev": <row>, "after": n }`.
    pub fn to_details(&self) -> Value {
        json!({ "jev": self.row, "after": self.after })
    }

    /// The update a partial's details carry, if they are one.
    pub fn from_details(details: &Value) -> Option<Self> {
        Some(Self {
            row: details.get("jev")?.clone(),
            after: usize::try_from(details.get("after")?.as_u64()?).ok()?,
        })
    }
}

/// A reserved infer call's row. Its nested ToolEnd loses details in the
/// live view, so Codemode reports its own final row as well.
#[derive(Debug, Clone, PartialEq)]
pub struct InferUpdate {
    pub row: Value,
}

impl InferUpdate {
    pub fn new(row: &CallRow) -> Self {
        Self { row: row.to_json() }
    }

    pub fn to_details(&self) -> Value {
        json!({ "infer": self.row })
    }

    pub fn from_details(details: &Value) -> Option<Self> {
        Some(Self {
            row: details.get("infer")?.clone(),
        })
    }
}
