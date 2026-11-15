//! The errors the loop's seams return: [`ToolError`] from a tool,
//! [`PluginError`] from a plugin or a hook.
//!
//! Each converts with `?` from the errors that commonly cross its seam,
//! and from a message (`String` or `&str`). Anything else goes in as a
//! boxed error, through [`ToolError::other`] or [`PluginError::other`].

use std::{error::Error, io};

use heiko_store::StoreError;

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
    #[error("{}", .0.text_content())]
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
/// whose text is already there is left out: most messages here carry
/// their source's after a `: `, and some are only their source's. A
/// message that merely ends with the same words does not carry it.
pub fn describe(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        let part = next.to_string();
        let carried = text == part
            || text
                .strip_suffix(part.as_str())
                .is_some_and(|before| before.ends_with(": "));
        if !carried {
            text.push_str(": ");
            text.push_str(&part);
        }
        source = next.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use hegel::{TestCase, generators as gs};

    use super::*;

    /// How a layer of a chain writes itself.
    #[derive(Debug, Clone, Copy)]
    enum Shows {
        /// Its own message only.
        Own,
        /// Its own message, then its source's: `"{own}: {source}"`.
        Carries,
        /// Its source's message only, as `"{0}"` over a source does.
        Source,
    }

    /// One layer of an error chain.
    #[derive(Debug)]
    struct Layer {
        own: String,
        shows: Shows,
        source: Option<Box<Layer>>,
    }

    impl std::fmt::Display for Layer {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match (self.shows, &self.source) {
                (Shows::Carries, Some(source)) => {
                    write!(f, "{}: {source}", self.own)
                }
                (Shows::Source, Some(source)) => write!(f, "{source}"),
                _ => f.write_str(&self.own),
            }
        }
    }

    impl Error for Layer {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            self.source.as_deref().map(|layer| layer as _)
        }
    }

    /// A chain of `layers`, outermost first; the last has no source.
    fn chain(layers: &[(String, Shows)]) -> Layer {
        let mut source = None;
        for (own, shows) in layers.iter().rev() {
            source = Some(Box::new(Layer {
                own: own.clone(),
                shows: *shows,
                source,
            }));
        }
        *source.expect("a layer at least")
    }

    /// What a chain says: each layer's own message once, outermost
    /// first, but for the ones that only show their source's.
    fn said(layers: &[(String, Shows)]) -> String {
        let last = layers.len() - 1;
        layers
            .iter()
            .enumerate()
            .filter(|(i, (_, shows))| {
                *i == last || !matches!(shows, Shows::Source)
            })
            .map(|(_, (own, _))| own.as_str())
            .collect::<Vec<_>>()
            .join(": ")
    }

    /// Any chain, up to six deep, each layer carrying its source's
    /// message or not, reads as each layer's message once. Messages are
    /// distinct and without `": "`, and often end as another one does
    /// (`"the disk is full"` over `"full"`), which must not hide it.
    #[hegel::test(test_cases = 500)]
    fn a_chain_names_each_layer_once(tc: TestCase) {
        let owns: Vec<String> = tc.draw(
            gs::vecs(hegel::one_of!(
                gs::sampled_from(
                    [
                        "full",
                        "is full",
                        "disk is full",
                        "the disk is full",
                        "disk full",
                        "notes",
                        "the notes",
                        "reading the notes",
                    ]
                    .map(String::from)
                    .to_vec()
                ),
                gs::text().alphabet("ab :").min_size(1).max_size(6),
            ))
            .min_size(1)
            .max_size(6)
            .unique(true),
        );
        tc.assume(owns.iter().all(|own| !own.contains(": ")));
        let layers: Vec<(String, Shows)> = owns
            .into_iter()
            .map(|own| {
                let shows = [Shows::Own, Shows::Carries, Shows::Source]
                    [tc.draw(gs::integers::<usize>().max_value(2))];
                (own, shows)
            })
            .collect();
        let error = chain(&layers);
        assert_eq!(describe(&error), said(&layers), "{layers:?}");
        // At a seam, a boxed error reads the same.
        let seam = PluginError::other(chain(&layers));
        assert_eq!(describe(&seam), said(&layers));
    }

    #[derive(Debug, thiserror::Error)]
    #[error("the disk is full")]
    struct Full(#[source] io::Error);

    /// Found by [`a_chain_names_each_layer_once`]: a message that only
    /// ends as its source's does, without carrying it, still names it.
    #[test]
    fn a_message_ending_as_its_source_still_names_it() {
        let error = Full(io::Error::other("full"));
        assert_eq!(describe(&error), "the disk is full: full");
    }
}
