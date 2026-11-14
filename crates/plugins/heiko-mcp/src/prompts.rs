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

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::{info::PromptInfo, names::tool_names};

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
