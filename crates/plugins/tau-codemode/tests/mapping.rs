//! The plugin's rules that hold for every input: the value a script
//! gets from a tool's result, and the namespaces a script waits for.

use hegel::generators as gs;
use serde_json::{Value, json};
use tau_agent::tool::{ToolError, ToolOutput};
use tau_ai::message::{ImageContent, InputBlock, TextContent};
use tau_codemode::{
    ToolReply,
    plugin::{Wanted, script_reply, wanted},
};

/// One block of a tool's output.
#[derive(Debug, Clone)]
enum Block {
    Text(String),
    Image,
}
hegel::pretty_print_as_debug!(Block);

#[hegel::composite]
fn blocks(tc: &hegel::TestCase) -> Vec<Block> {
    let n: usize = tc.draw(gs::integers().max_value(4_usize));
    (0..n)
        .map(|_| {
            if tc.draw(gs::booleans()) {
                Block::Text(tc.draw(gs::text().max_size(16)))
            } else {
                Block::Image
            }
        })
        .collect()
}

fn output(blocks: &[Block], structured: Option<Value>) -> ToolOutput {
    ToolOutput {
        content: blocks
            .iter()
            .map(|block| match block {
                Block::Text(text) => InputBlock::Text(TextContent {
                    text: text.clone(),
                    text_signature: None,
                }),
                Block::Image => InputBlock::Image(ImageContent {
                    data: "AAAA".into(),
                    mime_type: "image/png".into(),
                }),
            })
            .collect(),
        details: Some(json!({ "ignored": true })),
        structured,
    }
}

/// How a call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    Ok,
    /// A failure with output of its own.
    Output,
    /// A failure with only a message.
    Message,
}
hegel::pretty_print_as_debug!(Ending);

/// The script gets the structured output exactly when the tool has an
/// output schema and the output carries one, whether the call failed
/// or not; else a success's text blocks, one a line, without images;
/// else an error with the text the loop's error shows.
#[hegel::test(test_cases = 300)]
fn a_result_maps_to_one_script_value(tc: hegel::TestCase) {
    let blocks: Vec<Block> = tc.draw(blocks());
    let structured: Option<Value> =
        tc.draw(gs::optional(hegel::extras::serde_json::values()));
    let has_schema: bool = tc.draw(gs::booleans());
    let ending: Ending = tc.draw(gs::sampled_from(vec![
        Ending::Ok,
        Ending::Output,
        Ending::Message,
    ]));
    let joined: String = blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.as_str()),
            Block::Image => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let message: String = tc.draw(gs::text().max_size(16));
    let result = match ending {
        Ending::Ok => Ok(output(&blocks, structured.clone())),
        Ending::Output => {
            Err(ToolError::output(output(&blocks, structured.clone())))
        }
        Ending::Message => Err(ToolError::Message(message.clone())),
    };
    let display = result.as_ref().err().map(ToString::to_string);
    let value = script_reply(has_schema, result);
    match (ending, has_schema, structured) {
        (Ending::Message, ..) => assert_eq!(value, Err(message)),
        (_, true, Some(structured)) => assert_eq!(
            value,
            Ok(ToolReply {
                value: structured,
                error: (ending == Ending::Output).then_some(joined),
                usage: None,
                usage_complete: None,
            })
        ),
        (Ending::Ok, ..) => {
            assert_eq!(value, Ok(ToolReply::success(Value::String(joined))))
        }
        (Ending::Output, ..) => {
            assert_eq!(value, Err(joined));
            assert_eq!(value.unwrap_err(), display.unwrap());
        }
    }
}

/// Payload fields cannot override execution status; empty diagnostics
/// and a null structured value must still preserve a failed call.
#[test]
fn structured_fields_do_not_determine_call_status() {
    let success = script_reply(
        true,
        Ok(ToolOutput {
            structured: Some(json!({"isError": true})),
            ..ToolOutput::text("fine")
        }),
    )
    .unwrap();
    assert_eq!(success.error, None);
    for value in [Value::Null, json!({"isError": false})] {
        let reply = script_reply(
            true,
            Err(ToolError::output(ToolOutput {
                structured: Some(value.clone()),
                ..ToolOutput::text("")
            })),
        )
        .unwrap();
        assert_eq!(reply.value, value);
        assert_eq!(reply.error.as_deref(), Some(""));
    }
}

#[test]
fn ordinary_tool_payload_cannot_claim_inference_usage() {
    let claimed = json!({"cost": {"total": 99.0}, "input": 2, "output": 0,
        "cacheRead": 0, "cacheWrite": 0, "totalTokens": 2});
    let reply = script_reply(
        true,
        Ok(ToolOutput {
            structured: Some(json!({"usage": claimed, "ok": true})),
            details: Some(json!({"usage": claimed, "usage_complete": true})),
            ..ToolOutput::text("ok")
        }),
    )
    .unwrap();
    assert_eq!(reply.usage, None);
    assert_eq!(reply.usage_complete, None);
}

#[hegel::composite]
fn segment(tc: &hegel::TestCase) -> String {
    // No `__` inside, none at either end: a sanitized server or tool
    // name.
    let parts: Vec<String> = tc.draw(
        gs::vecs(gs::from_regex("[a-z0-9]{1,6}"))
            .min_size(1)
            .max_size(3),
    );
    parts.join("_")
}

/// A script that names `tools.mcp__<server>__<tool>` waits for
/// `mcp__<server>`, and for nothing outside the servers it names; one
/// that uses a discovery global waits for every namespace.
#[hegel::test(test_cases = 200)]
fn a_script_waits_for_the_servers_it_names(tc: hegel::TestCase) {
    let calls: Vec<(String, String)> =
        tc.draw(gs::vecs(hegel::tuples!(segment(), segment())).max_size(4));
    let discovery: Option<&str> =
        tc.draw(gs::optional(gs::sampled_from(vec![
            "search_tools('x')",
            "describe_namespace('mcp__a')",
            "#ALL_TOOLS",
        ])));
    let mut code: String = calls
        .iter()
        .map(|(server, tool)| {
            format!("tools.mcp__{server}__{tool}({{}})\nlocal xmcp__no = 1\n")
        })
        .collect();
    if let Some(global) = discovery {
        code.push_str(&format!("return {global}\n"));
    }
    match (discovery, wanted(&code)) {
        (Some(_), got) => assert_eq!(got, Wanted::All),
        (None, Wanted::Nothing) => assert!(calls.is_empty()),
        (None, Wanted::Namespaces(names)) => {
            for (server, _) in &calls {
                assert!(names.contains(&format!("mcp__{server}")), "{names:?}");
            }
            for name in &names {
                let server = name.strip_prefix("mcp__").unwrap();
                assert!(
                    calls.iter().any(|(s, t)| format!("{s}__{t}")
                        .starts_with(&format!("{server}__"))),
                    "{name} is not named in {code}"
                );
            }
        }
        (None, Wanted::All) => panic!("no discovery global in {code}"),
    }
}
