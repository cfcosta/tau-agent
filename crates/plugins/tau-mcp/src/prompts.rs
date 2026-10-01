//! Servers' prompts as composer commands (`docs/reference/mcp.md`,
//! "Prompts"): `/mcp__<server>__<prompt> key=value ...` gets the prompt
//! with those arguments and puts its messages in the composer.
//!
//! - [`command_names`]: the commands' names, as tools are named.
//! - [`parse_arguments`] and [`format_arguments`]: `key=value` pairs,
//!   values quoted with `"` (`\"` and `\\` inside) or `'` when they have
//!   spaces or quotes.
//! - [`check_arguments`]: every required argument given, none unknown,
//!   none twice.
//! - [`prompt_text`]: a `GetPromptResult`'s messages as the text a person
//!   sends.

use std::collections::BTreeSet;
#[cfg(feature = "host")]
use std::sync::Arc;

use serde_json::{Map, Value};
#[cfg(feature = "host")]
use tokio_util::sync::CancellationToken;

#[cfg(feature = "host")]
use crate::connection::Connection;
#[cfg(feature = "host")]
use crate::results::resource_link;
use crate::{info::PromptInfo, names::tool_names};

/// A server's prompt, with the command that gets it.
#[cfg(feature = "host")]
#[derive(Clone)]
pub struct Prompt {
    /// Without the slash: `mcp__<server>__<prompt>`.
    pub command: String,
    pub info: PromptInfo,
    pub connection: Arc<Connection>,
}

/// Every prompt `connections` offer now, with its command.
#[cfg(feature = "host")]
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

#[cfg(feature = "host")]
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

/// The commands of `prompts`, given as `(server, prompt)`, in the same
/// order: `mcp__<server>__<prompt>`, named as tools are
/// ([`tool_names`]), so they fit in 64 characters and never collide.
pub fn command_names(prompts: &[(&str, &str)]) -> Vec<String> {
    tool_names(prompts)
}

/// The `key=value` pairs of `text`, in order. A value may be quoted, in
/// whole or in part, with `"..."` (where `\` takes the next character as
/// it is) or `'...'` (taken as it is); outside quotes, whitespace ends
/// it.
pub fn parse_arguments(text: &str) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        while chars.next_if(|c| c.is_whitespace()).is_some() {}
        if chars.peek().is_none() {
            return Ok(pairs);
        }
        let mut key = String::new();
        while let Some(c) = chars.next_if(|c| *c != '=' && !c.is_whitespace()) {
            key.push(c);
        }
        if chars.next_if_eq(&'=').is_none() {
            return Err(format!(
                "`{key}` is not an argument: write key=value, quoting values with spaces"
            ));
        }
        if key.is_empty() {
            return Err("an argument has no name before its `=`".to_owned());
        }
        let mut value = String::new();
        while let Some(c) = chars.next_if(|c| !c.is_whitespace()) {
            match c {
                '"' => loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(next) => value.push(next),
                            None => return Err(unclosed(&key)),
                        },
                        Some(other) => value.push(other),
                        None => return Err(unclosed(&key)),
                    }
                },
                '\'' => loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(other) => value.push(other),
                        None => return Err(unclosed(&key)),
                    }
                },
                other => value.push(other),
            }
        }
        pairs.push((key, value));
    }
}

fn unclosed(key: &str) -> String {
    format!("the quote in the value of `{key}` is not closed")
}

/// `pairs` as [`parse_arguments`] reads them back: a value with
/// whitespace, a quote or a `\`, or an empty one, in double quotes.
pub fn format_arguments(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            let plain = !value.is_empty()
                && !value.chars().any(|c| {
                    c.is_whitespace() || matches!(c, '"' | '\'' | '\\')
                });
            if plain {
                format!("{key}={value}")
            } else {
                let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
                format!("{key}=\"{escaped}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// How `command` is written: `/command name=<name> [style=<style>]`,
/// required arguments first.
pub fn usage(command: &str, prompt: &PromptInfo) -> String {
    let mut text = format!("/{command}");
    let required = prompt.arguments.iter().filter(|a| a.required);
    let optional = prompt.arguments.iter().filter(|a| !a.required);
    for argument in required {
        text.push_str(&format!(" {0}=<{0}>", argument.name));
    }
    for argument in optional {
        text.push_str(&format!(" [{0}=<{0}>]", argument.name));
    }
    text
}

/// What the composer's menu shows after a command's name: its arguments,
/// `name=… [style=…]`.
pub fn arguments_hint(prompt: &PromptInfo) -> String {
    let required = prompt.arguments.iter().filter(|a| a.required);
    let optional = prompt.arguments.iter().filter(|a| !a.required);
    required
        .map(|a| format!("{}=…", a.name))
        .chain(optional.map(|a| format!("[{}=…]", a.name)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The arguments `prompt`, run as `command`, gets from `pairs`, or why
/// they will not do: one it does not take, one given twice, or a
/// required one missing.
pub fn check_arguments(
    command: &str,
    prompt: &PromptInfo,
    pairs: &[(String, String)],
) -> Result<Map<String, Value>, String> {
    let known: BTreeSet<&str> =
        prompt.arguments.iter().map(|a| a.name.as_str()).collect();
    let mut given = Map::new();
    for (key, value) in pairs {
        if !known.contains(key.as_str()) {
            return Err(if known.is_empty() {
                format!("/{command} takes no arguments, but `{key}` was given.")
            } else {
                format!(
                    "/{command} has no argument `{key}`. Usage: {}",
                    usage(command, prompt)
                )
            });
        }
        if given
            .insert(key.clone(), Value::String(value.clone()))
            .is_some()
        {
            return Err(format!("`{key}` is given twice."));
        }
    }
    let missing: Vec<String> = prompt
        .arguments
        .iter()
        .filter(|a| a.required && !given.contains_key(&a.name))
        .map(|a| format!("`{}`", a.name))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "/{command} needs {} {}. Usage: {}",
            if missing.len() == 1 {
                "the argument"
            } else {
                "the arguments"
            },
            missing.join(", "),
            usage(command, prompt)
        ));
    }
    Ok(given)
}

#[cfg(feature = "host")]
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

#[cfg(feature = "host")]
fn field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

#[cfg(feature = "host")]
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
