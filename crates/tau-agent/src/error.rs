//! The errors the loop's seams return: [`ToolError`] from a tool,
//! [`PluginError`] from a plugin or a hook.
//!
//! Each converts with `?` from the errors that commonly cross its seam,
//! and from a message (`String` or `&str`). Anything else goes in as a
//! boxed error, through [`ToolError::other`] or [`PluginError::other`].

use std::{error::Error, io};

use tau_ai::message::InputBlock;
use tau_store::StoreError;

use crate::{agent::SubAgentError, plugin::AskError, tool::ToolOutput};

/// Any error, boxed: what the seams' catch-all variants hold.
pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// Why a tool call failed. The model sees its `Display`
/// (`docs/reference/agent-loop.md`, "Tool execution").
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    SubAgent(#[from] SubAgentError),
    /// A tool's blocking task panicked or was cancelled.
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Other(#[from] BoxError),
    /// A failure with output of its own, such as a command that exited
    /// non-zero: the model sees the output's text as the error, and its
    /// `details` reach events and the stored result as a success's do.
    #[error("{}", text_of(.0))]
    Output(Box<ToolOutput>),
}

impl ToolError {
    /// Any other error.
    pub fn other(error: impl Into<BoxError>) -> Self {
        Self::Other(error.into())
    }

    /// A failure that keeps `output`, details and all.
    pub fn output(output: ToolOutput) -> Self {
        Self::Output(Box::new(output))
    }
}

/// The text blocks of `output`, one after another.
fn text_of(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            InputBlock::Text(text) => Some(text.text.as_str()),
            InputBlock::Image(_) => None,
        })
        .collect()
}

impl From<String> for ToolError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for ToolError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

/// Why a plugin or a hook failed at a seam. The loop reports it as
/// `RunEvent::PluginError`, with its whole chain of sources.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Ask(#[from] AskError),
    /// A plugin's blocking task panicked or was cancelled.
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error(transparent)]
    Other(#[from] BoxError),
}

impl PluginError {
    /// Any other error.
    pub fn other(error: impl Into<BoxError>) -> Self {
        Self::Other(error.into())
    }
}

impl From<String> for PluginError {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for PluginError {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

/// `error` and its sources, joined with `: `, outermost first. A source
/// whose text already ends what came before it is left out: most
/// messages here carry their source's text already.
pub fn describe(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        let part = next.to_string();
        if !text.ends_with(&part) {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = next.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    enum Outer {
        #[error("reading the notes: {0}")]
        Carried(#[from] io::Error),
    }

    #[derive(Debug, thiserror::Error)]
    #[error("reading the notes")]
    struct Bare(#[source] io::Error);

    #[test]
    fn a_chain_names_each_source_once() {
        let io = || io::Error::other("disk full");
        assert_eq!(
            describe(&Outer::from(io())),
            "reading the notes: disk full"
        );
        assert_eq!(describe(&Bare(io())), "reading the notes: disk full");
        let seam = PluginError::other(Outer::from(io()));
        assert_eq!(describe(&seam), "reading the notes: disk full");
    }
}
