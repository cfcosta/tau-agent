//! An MCP tool's card: the server and the tool, the tool's hints, and
//! what it returned, as structured JSON when the server gave some, else
//! as text. Its head and body are view trees (ADR 0045).

use serde_json::Value;
use tau_ui_kit::theme::Tone;
use tau_ui_plugin::{
    CallData,
    ViewCx,
    points::{AtCard, CardView},
    tree::Node,
};

use super::{
    McpUi,
    page::{hint_badge, hints},
};
use crate::info::Annotations;

/// The most lines of a result's text a card shows.
const TEXT_LINES: usize = 40;

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
    let ignore = |_: &str, _: &Value, _: &mut gpui::App| {};
    let head = view.tree(
        &format!("mcp-card-head-{}", at.call_id),
        &head(&shown, &at.summary),
        ignore,
    );
    let body = view.tree(
        &format!("mcp-card-body-{}", at.call_id),
        &body(&at.data, &shown),
        ignore,
    );
    Some(CardView {
        head: Some(head),
        label: None,
        failed: shown.failed.clone(),
        edge: shown.failed.as_ref().map(|_| Tone::Danger),
        shape: None,
        body: Some(body),
        folds: at.data.result.is_some(),
        inset: false,
    })
}

/// The server and the tool, and the tool's hints.
pub fn head(shown: &Shown, summary: &str) -> Node {
    let named = match (&shown.server, &shown.tool) {
        (Some(server), Some(tool)) => format!("{server} · {tool}"),
        _ => summary.to_owned(),
    };
    let mut head = vec![Node::Mono { text: named }];
    head.extend(hints(&shown.annotations).into_iter().map(hint_badge));
    Node::row(head)
}

/// What the call sent, and what it returned.
pub fn body(data: &CallData, shown: &Shown) -> Node {
    let heading = |text: &str| Node::Heading { text: text.into() };
    let json = |text: String| Node::Code {
        lang: Some("json".into()),
        text,
    };
    let mut body = Vec::new();
    if !data.args.is_null()
        && data.args.as_object().is_none_or(|args| !args.is_empty())
    {
        body.push(heading("Arguments"));
        body.push(json(
            serde_json::to_string_pretty(&data.args).unwrap_or_default(),
        ));
    }
    if let Some(structured) = &shown.structured {
        body.push(heading("Structured result"));
        body.push(json(structured.clone()));
    }
    if let Some(text) = &shown.text {
        let lines: Vec<&str> = text.lines().collect();
        let more = lines.len().saturating_sub(TEXT_LINES);
        body.push(heading("Result"));
        body.push(Node::Mono {
            text: lines[..lines.len() - more].join("\n"),
        });
        if more > 0 {
            body.push(Node::text(format!("… {more} more lines")));
        }
    }
    if data.result.is_none()
        && let Some(partial) = &data.partial
    {
        body.push(Node::Mono {
            text: partial.clone(),
        });
    }
    Node::stack(body)
}
