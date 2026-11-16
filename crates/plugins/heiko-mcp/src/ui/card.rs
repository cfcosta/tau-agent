//! An MCP tool's card: what it sent on the call's line with the tool's
//! hints as stamps, the server as the result's word, and what it
//! returned on a band, as structured JSON when the server gave some,
//! else as text.
//! Its head and body are view trees (ADR 0045).

use heiko_ui_kit::theme::Tone;
use heiko_ui_plugin::{
    CallData,
    ViewCx,
    points::{AtCard, CardView},
    tree::{Ink, Node},
};
use serde_json::Value;

use super::{McpUi, page::hints};
use crate::info::Annotations;

/// The most lines of a result a card shows.
const TEXT_LINES: usize = 40;

/// The most characters of its arguments a call's line holds; longer
/// ones go in the body, whole.
const ARGS_CHARS: usize = 96;

/// What a card shows of a call to an MCP tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    pub server: Option<String>,
    pub tool: Option<String>,
    pub annotations: Annotations,
    /// The result's structured content, pretty.
    pub structured: Option<String>,
    /// The result's text, when it has no structured content.
    pub text: Option<String>,
    /// The first line of the result, when the call failed.
    pub failed: Option<String>,
}

/// What the card of `data` shows: from the result's details once the
/// call ended, else from what the repository's servers list
/// (`listed`: server, tool and hints), so a call going on names its
/// server too.
pub fn shown(
    data: &CallData,
    listed: Option<(String, String, Annotations)>,
) -> Shown {
    let details = data.result.as_ref().and_then(|r| r.details.as_ref());
    let field = |key: &str| {
        details
            .and_then(|details| details.get(key))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let (listed_server, listed_tool, listed_hints) = match listed {
        Some((server, tool, hints)) => (Some(server), Some(tool), Some(hints)),
        None => (None, None, None),
    };
    let annotations = details
        .and_then(|details| details.get("annotations"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .or(listed_hints)
        .unwrap_or_default();
    let structured = details
        .and_then(|details| details.get("structuredContent"))
        .map(|value| serde_json::to_string_pretty(value).unwrap_or_default());
    let result = data.result.as_ref();
    Shown {
        server: field("server").or(listed_server),
        tool: field("tool").or(listed_tool),
        annotations,
        text: result
            .filter(|_| structured.is_none())
            .map(|result| result.text.clone())
            .filter(|text| !text.is_empty()),
        structured,
        failed: result.filter(|result| result.error).map(|result| {
            result.text.lines().next().unwrap_or_default().to_owned()
        }),
    }
}

/// The card of a call to an MCP tool: any tool named `mcp__…`.
pub fn card(at: &AtCard, view: &mut ViewCx<'_, McpUi>) -> Option<CardView> {
    if !at.tool.starts_with("mcp__") {
        return None;
    }
    let servers = view.repo(&at.run.repo).unwrap_or(view.data);
    let listed = servers.tool(&at.tool).map(|(server, tool)| {
        (server.name.clone(), tool.tool.clone(), tool.annotations)
    });
    let shown = shown(&at.data, listed);
    let body = body(&at.data, &shown);
    Some(CardView {
        head: Some(head(&at.data, &shown, &at.summary)),
        label: shown.server.clone(),
        failed: shown.failed.clone(),
        edge: shown.failed.as_ref().map(|_| Tone::Danger),
        shape: None,
        body,
        folds: false,
        inset: false,
        kind: Some(heiko_ui_kit::theme::ToolKind::Reach),
        waits: false,
    })
}

/// The call's arguments on one line, when it sent any.
fn args_line(data: &CallData) -> Option<String> {
    (!data.args.is_null()
        && data.args.as_object().is_none_or(|args| !args.is_empty()))
    .then(|| data.args.to_string())
}

/// What the call sent, on one line (`summary` when it sent nothing or
/// too much for a line), and the tool's hints.
pub fn head(data: &CallData, shown: &Shown, summary: &str) -> Node {
    let args = args_line(data)
        .filter(|args| args.chars().count() <= ARGS_CHARS)
        .unwrap_or_else(|| summary.to_owned());
    let mut head = vec![Node::Mono { text: args }];
    head.extend(hints(&shown.annotations).into_iter().map(hint_stamp));
    Node::row(head)
}

/// A hint's stamp: destructive in red, read-only in green.
fn hint_stamp(hint: &'static str) -> Node {
    Node::Stamp {
        label: hint.into(),
        tone: match hint {
            "destructive" => Ink::Bad,
            "read-only" => Ink::Good,
            _ => Ink::Neutral,
        },
    }
}

/// `text` on a band, down to its first [`TEXT_LINES`] lines, and how
/// many more there are.
fn band(lang: Option<&str>, text: &str) -> Vec<Node> {
    let lines: Vec<&str> = text.lines().collect();
    let more = lines.len().saturating_sub(TEXT_LINES);
    let mut pieces = vec![Node::Code {
        lang: lang.map(str::to_owned),
        text: lines[..lines.len() - more].join("\n"),
        first_line: None,
    }];
    if more > 0 {
        pieces.push(Node::Toned {
            text: format!("+ {more} more lines"),
            tone: heiko_ui_plugin::tree::Tone::Neutral,
            mono: true,
        });
    }
    pieces
}

/// What the call returned, or has said so far; before it, what it sent
/// when that is too long for its line.
pub fn body(data: &CallData, shown: &Shown) -> Option<Node> {
    let mut body = Vec::new();
    if args_line(data).is_some_and(|args| args.chars().count() > ARGS_CHARS) {
        body.extend(band(
            Some("json"),
            &serde_json::to_string_pretty(&data.args).unwrap_or_default(),
        ));
    }
    if let Some(structured) = &shown.structured {
        body.extend(band(Some("json"), structured));
    }
    if let Some(text) = &shown.text {
        body.extend(band(None, text));
    }
    if data.result.is_none()
        && let Some(partial) = &data.partial
    {
        body.extend(band(None, partial));
    }
    (!body.is_empty()).then(|| Node::stack(body))
}
