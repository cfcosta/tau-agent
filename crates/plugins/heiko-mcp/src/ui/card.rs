//! An MCP tool's step (ADR 0064): `server · tool` for its subject, the
//! tool's hints as stamps in its receipt, and for its body what it
//! returned, as structured JSON when the server gave some, else as
//! text; before it returns, what it sent.

use heiko_ui_plugin::{
    CallData,
    ViewCx,
    points::AtCard,
    step::{Body, Receipt, Step},
    tree::{Ink, KeyValue},
};
use serde_json::Value;

use super::{McpUi, page::hints};
use crate::info::Annotations;

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

/// The step of a call to an MCP tool: any tool named `mcp__…`.
pub fn card(at: &AtCard, view: &mut ViewCx<'_, McpUi>) -> Option<Step> {
    if !at.tool.starts_with("mcp__") {
        return None;
    }
    let servers = view.repo(&at.run.repo).unwrap_or(view.data);
    let listed = servers.tool(&at.tool).map(|(server, tool)| {
        (server.name.clone(), tool.tool.clone(), tool.annotations)
    });
    let shown = shown(&at.data, listed);
    let subject = match (&shown.server, &shown.tool) {
        (Some(server), Some(tool)) => Some(format!("{server} · {tool}")),
        (server, tool) => server.clone().or_else(|| tool.clone()),
    };
    Some(Step {
        subject,
        receipt: hints(&shown.annotations).into_iter().map(hint).collect(),
        failed: shown.failed.clone(),
        body: body(&at.data, &shown),
        kind: Some(heiko_ui_kit::theme::ToolKind::Reach),
        ..Step::default()
    })
}

/// A hint's stamp: destructive in red, read-only in green.
fn hint(hint: &'static str) -> Receipt {
    Receipt::stamp(
        hint,
        match hint {
            "destructive" => Ink::Bad,
            "read-only" => Ink::Good,
            _ => Ink::Neutral,
        },
    )
}

/// What the call returned, or has said so far; before it, what it sent,
/// each argument by its name.
pub fn body(data: &CallData, shown: &Shown) -> Option<Body> {
    if let Some(structured) = &shown.structured {
        return Some(Body::Code {
            lang: Some("json".into()),
            text: structured.clone(),
            first_line: None,
        });
    }
    if let Some(text) = &shown.text {
        return Some(Body::Code {
            lang: None,
            text: text.clone(),
            first_line: None,
        });
    }
    if data.result.is_none()
        && let Some(partial) = &data.partial
    {
        return Some(Body::Output {
            lines: partial.lines().map(str::to_owned).collect(),
        });
    }
    let pairs: Vec<KeyValue> = data
        .args
        .as_object()?
        .iter()
        .map(|(key, value)| KeyValue {
            key: key.clone(),
            value: match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            },
        })
        .collect();
    (!pairs.is_empty()).then_some(Body::Record { pairs })
}
