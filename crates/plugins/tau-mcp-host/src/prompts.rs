//! Servers' prompts as composer commands: what a command reads is the
//! interface half's (`tau_mcp::prompts`); getting a prompt from its
//! server is the host's.
//!
//! - [`prompt_text`]: a `GetPromptResult`'s messages as the text a person
//!   sends.

use std::sync::Arc;

use serde_json::Value;
use tau_mcp::info::PromptInfo;
pub use tau_mcp::prompts::*;
use tokio_util::sync::CancellationToken;

use crate::{connection::Connection, results::resource_link};

/// A server's prompt, with the command that gets it.
#[derive(Clone)]
pub struct Prompt {
    /// Without the slash: `mcp__<server>__<prompt>`.
    pub command: String,
    pub info: PromptInfo,
    pub connection: Arc<Connection>,
}

/// Every prompt `connections` offer now, with its command.
pub fn prompts(connections: &[Arc<Connection>]) -> Vec<Prompt> {
    let listed: Vec<(Arc<Connection>, PromptInfo)> = connections
        .iter()
        .flat_map(|connection| {
            connection
                .prompts()
                .into_iter()
                .map(move |info| (connection.clone(), info))
        })
        .collect();
    let pairs: Vec<(&str, &str)> = listed
        .iter()
        .map(|(connection, info)| (connection.name(), info.name.as_str()))
        .collect();
    let names = command_names(&pairs);
    listed
        .into_iter()
        .zip(names)
        .map(|((connection, info), command)| Prompt {
            command,
            info,
            connection,
        })
        .collect()
}

impl Prompt {
    /// Gets the prompt with the `key=value` pairs of `arguments`, checked
    /// first, and gives its messages as text.
    pub async fn get(
        &self,
        arguments: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let pairs = parse_arguments(arguments)?;
        let given = check_arguments(&self.command, &self.info, &pairs)?;
        let result = self
            .connection
            .get_prompt(&self.info.name, given, cancel)
            .await?;
        let text = prompt_text(self.connection.name(), &result);
        if text.trim().is_empty() {
            return Err(format!("/{} gave no messages", self.command));
        }
        Ok(text)
    }
}

/// A `GetPromptResult`'s messages as the text a person sends, one after
/// another with a blank line between: text as it is, an embedded text
/// resource as its text, a resource link as a call's result shows it,
/// and images, audio and binary resources named in brackets.
pub fn prompt_text(server: &str, result: &Value) -> String {
    let messages = result
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    messages
        .iter()
        .flat_map(|message| match message.get("content") {
            Some(Value::Array(blocks)) => blocks.clone(),
            Some(block) => vec![block.clone()],
            None => Vec::new(),
        })
        .map(|block| block_text(server, &block))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn block_text(server: &str, block: &Value) -> String {
    let mime = || field(block, "mimeType").unwrap_or("unknown type");
    match field(block, "type") {
        Some("text") => field(block, "text").unwrap_or_default().to_owned(),
        Some("image") => format!("[Image ({})]", mime()),
        Some("audio") => format!("[Audio ({})]", mime()),
        Some("resource_link") => resource_link(server, block),
        Some("resource") => {
            let resource = block.get("resource").unwrap_or(&Value::Null);
            match field(resource, "text") {
                Some(text) => text.to_owned(),
                None => format!(
                    "[Resource {} ({})]",
                    field(resource, "uri").unwrap_or_default(),
                    field(resource, "mimeType")
                        .unwrap_or("application/octet-stream")
                ),
            }
        }
        _ => block.to_string(),
    }
}
