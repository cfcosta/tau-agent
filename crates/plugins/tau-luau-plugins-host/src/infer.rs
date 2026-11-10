//! `infer` in a run hook (ADR 0050): its arguments, a request a plugin
//! writes itself, and the answers it refuses.

use serde_json::{Value, json};
use tau_ai::{message::AssistantMessage, responses::request::ReasoningEffort};

/// `infer`'s arguments in a run hook: codemode's, and `conversation`.
pub fn parameters() -> Value {
    let mut parameters =
        tau_codemode_host::inference::InferRequest::parameters();
    parameters["properties"]["conversation"] = json!({
        "description": "In rewrite_context: true sends the run's conversation \
                        before the task, or a number its first messages."
    });
    parameters["properties"]["instructions"] = json!({
        "description": "The request's own instructions: the task goes as \
                        written, and the answer is its text."
    });
    parameters
}

/// Why `infer` gave no answer, and, for an answer it refused, the
/// `reason`: `"error"`, `"length"` or `"tool_call"`.
#[derive(Debug, PartialEq, Eq)]
pub struct Refused {
    pub error: String,
    pub reason: Option<&'static str>,
}

impl From<String> for Refused {
    fn from(error: String) -> Self {
        Self {
            error,
            reason: None,
        }
    }
}

impl From<&str> for Refused {
    fn from(error: &str) -> Self {
        error.to_owned().into()
    }
}

/// The text of `answer`, unless it stopped with an error, was cut off
/// at the model's output limit, or called a tool, in that order: an
/// answer a plugin must not take as whole. One that says an error, however
/// it stopped, is refused too.
pub fn answer_text(answer: &AssistantMessage) -> Result<String, Refused> {
    use tau_ai::message::StopReason;
    let refused = |error: &str, reason| Refused {
        error: error.to_owned(),
        reason: Some(reason),
    };
    let error = answer.error_message.as_deref();
    match answer.stop_reason {
        StopReason::Error => {
            return Err(refused(error.unwrap_or("Unknown error"), "error"));
        }
        StopReason::Length => {
            return Err(refused(
                "the answer hit the model's output limit and is incomplete",
                "length",
            ));
        }
        _ => {}
    }
    if answer.tool_calls().next().is_some() {
        return Err(refused("the model called a tool", "tool_call"));
    }
    if let Some(error) = error {
        return Err(refused(error, "error"));
    }
    Ok(answer.text())
}

/// How many of the run's messages `infer{ conversation }` sends first:
/// all of them for `true`; `None` when it names none.
pub(crate) fn conversation_of(
    value: Option<Value>,
) -> Result<Option<usize>, Refused> {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => Ok(None),
        Some(Value::Bool(true)) => Ok(Some(usize::MAX)),
        Some(count) => count
            .as_u64()
            .map(|count| Some(count as usize))
            .ok_or_else(|| {
                "conversation is true, or how many of the run's messages \
                 go first"
                    .into()
            }),
    }
}

/// An `infer` request a run hook writes itself, with its `instructions`
/// or in the run's `conversation`: the message goes as written, after the
/// earlier ones it gives, with no bound but the VM's on what it holds,
/// and the answer is its text.
pub(crate) struct Own {
    /// The task's texts, each a block, the context as JSON after them
    /// unless it is null.
    pub blocks: Vec<String>,
    /// What went before the task, in order: whether the person said it
    /// or the model, and its blocks.
    pub earlier: Vec<(bool, Vec<String>)>,
    pub instructions: Option<String>,
    /// How many of the run's messages go first.
    pub conversation: Option<usize>,
    pub effort: Option<ReasoningEffort>,
    pub model: Option<String>,
}

impl Own {
    pub(crate) fn parse(args: Value) -> Result<Self, Refused> {
        let Value::Object(mut fields) = args else {
            return Err("infer arguments must be an object".into());
        };
        if let Some(key) = fields.keys().find(|key| {
            !matches!(
                key.as_str(),
                "task"
                    | "context"
                    | "instructions"
                    | "conversation"
                    | "messages"
                    | "effort"
                    | "model"
            )
        }) {
            return Err(format!(
                "unknown argument of a request of the plugin's own: {key}"
            )
            .into());
        }
        let mut blocks = texts(fields.remove("task"))
            .filter(|blocks| blocks.iter().any(|text| !text.trim().is_empty()))
            .ok_or("task must be a text, or a list of texts, not blank")?;
        match fields.remove("context") {
            None | Some(Value::Null) => {}
            Some(context) => {
                let last = blocks.last_mut().expect("a task has a text");
                last.push_str("\n\n");
                last.push_str(&context.to_string());
            }
        }
        let earlier = match fields.remove("messages") {
            None | Some(Value::Null) => Vec::new(),
            // An empty list reaches the host as an empty object.
            Some(Value::Object(empty)) if empty.is_empty() => Vec::new(),
            Some(Value::Array(messages)) => messages
                .into_iter()
                .map(|message| {
                    let person = match message["role"].as_str() {
                        Some("user") => true,
                        Some("assistant") => false,
                        _ => return None,
                    };
                    Some((person, texts(Some(message["text"].clone()))?))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or(
                    "each of `messages` is { role = \"user\" or \
                     \"assistant\", text }",
                )?,
            Some(_) => return Err("messages must be a list".into()),
        };
        let instructions = match fields.remove("instructions") {
            None | Some(Value::Null) => None,
            Some(Value::String(instructions)) => Some(instructions),
            Some(_) => return Err("instructions must be a string".into()),
        };
        let conversation = conversation_of(fields.remove("conversation"))?;
        let effort = match fields.remove("effort") {
            None | Some(Value::Null) => None,
            Some(effort) => Some(serde_json::from_value(effort).map_err(
                |_| "effort is none, minimal, low, medium, high, xhigh or max",
            )?),
        };
        let model = match fields.remove("model") {
            None | Some(Value::Null) => None,
            Some(Value::String(model)) if !model.trim().is_empty() => {
                Some(model)
            }
            Some(_) => {
                return Err("model must be a model's id or family".into());
            }
        };
        if conversation.is_some()
            && (instructions.is_some()
                || effort.is_some()
                || model.is_some()
                || !earlier.is_empty()
                || blocks.len() > 1)
        {
            return Err("in the run's conversation, `infer` asks with the \
                        run's own settings, after its messages: no \
                        instructions, effort, model or messages, and one \
                        text"
                .into());
        }
        if fields.contains_key("schema") {
            return Err("a request of the plugin's own takes no schema".into());
        }
        Ok(Self {
            blocks,
            earlier,
            instructions,
            conversation,
            effort,
            model,
        })
    }
}

/// A text, or a list of texts, as blocks; `None` for anything else.
fn texts(value: Option<Value>) -> Option<Vec<String>> {
    match value? {
        Value::String(text) => Some(vec![text]),
        Value::Array(texts) if !texts.is_empty() => texts
            .into_iter()
            .map(|text| match text {
                Value::String(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

/// What the person (`person`) or the model said, as a message of
/// `blocks`: one text as it is, several as blocks.
pub(crate) fn said(
    person: bool,
    mut blocks: Vec<String>,
    now: tau_ai::message::Timestamp,
) -> tau_ai::message::Message {
    use tau_ai::message::{
        AssistantBlock,
        InputBlock,
        Message,
        StopReason,
        TextContent,
        UserContent,
        UserMessage,
    };
    let text = |text| TextContent {
        text,
        text_signature: None,
    };
    if !person {
        return Message::Assistant(AssistantMessage {
            content: blocks
                .into_iter()
                .map(|block| AssistantBlock::Text(text(block)))
                .collect(),
            model: String::new(),
            response_id: None,
            usage: Default::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: now,
        });
    }
    let content = if blocks.len() == 1 {
        UserContent::Text(blocks.remove(0))
    } else {
        UserContent::Blocks(
            blocks
                .into_iter()
                .map(|block| InputBlock::Text(text(block)))
                .collect(),
        )
    };
    Message::User(UserMessage {
        content,
        timestamp: now,
    })
}

/// What a round of `infer{ tools }` is told when it gives no
/// instructions of its own: codemode's say the model calls no tools.
pub const TOOL_INSTRUCTIONS: &str = "Carry out the user's task on its own, \
    calling the tools given as it needs them, and answer once it is done.";

/// One round of `infer{ tools }` (ADR 0050, gap L), as `tau.luau` asks
/// it: the request, the plugin's own tools it offers, and the rounds
/// before, each the model's answer as the host gave it and what each of
/// its calls answered. `tau.luau` calls the tools, with the hook's `ctx`.
pub(crate) struct Round {
    /// The task's texts, each a block, the context as JSON after them
    /// unless it is null: the message goes as written, as a request of
    /// the plugin's own does.
    pub blocks: Vec<String>,
    pub effort: Option<ReasoningEffort>,
    pub model: Option<String>,
    pub instructions: Option<String>,
    pub tools: Vec<tau_ai::responses::request::ToolDefinition>,
    pub turns: Vec<(AssistantMessage, Vec<Answered>)>,
}

/// What a tool call of a round answered.
pub(crate) struct Answered {
    pub id: String,
    pub name: String,
    pub text: String,
    pub ok: bool,
}

impl Round {
    pub(crate) fn parse(args: Value) -> Result<Self, Refused> {
        let Value::Object(mut fields) = args else {
            return Err("infer arguments must be an object".into());
        };
        let tools = match fields.remove("tools") {
            Some(Value::Array(tools)) => tools
                .into_iter()
                .map(|tool| {
                    Some(tau_ai::responses::request::ToolDefinition {
                        name: tool["name"].as_str()?.to_owned(),
                        description: tool["description"]
                            .as_str()
                            .unwrap_or_default()
                            .to_owned(),
                        parameters: tool["parameters"].clone(),
                        strict: false,
                    })
                })
                .collect::<Option<Vec<_>>>()
                .ok_or("each tool is { name, description, parameters }")?,
            _ => return Err("infer's `tools` is a list of tools".into()),
        };
        if tools.is_empty() {
            return Err("infer's `tools` names none".into());
        }
        let instructions = match fields.remove("instructions") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text),
            Some(_) => return Err("instructions must be a string".into()),
        };
        let turns = match fields.remove("turns") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Object(empty)) if empty.is_empty() => Vec::new(),
            Some(Value::Array(turns)) => turns
                .into_iter()
                .map(|turn| {
                    let message: AssistantMessage =
                        serde_json::from_value(turn["message"].clone()).ok()?;
                    let results = turn["results"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|result| {
                            Some(Answered {
                                id: result["id"].as_str()?.to_owned(),
                                name: result["name"].as_str()?.to_owned(),
                                text: result["text"]
                                    .as_str()
                                    .unwrap_or_default()
                                    .to_owned(),
                                ok: result["ok"].as_bool().unwrap_or(true),
                            })
                        })
                        .collect::<Option<Vec<_>>>()?;
                    Some((message, results))
                })
                .collect::<Option<Vec<_>>>()
                .ok_or("each turn is { message, results }")?,
            Some(_) => return Err("turns must be a list".into()),
        };
        if let Some(key) = fields.keys().find(|key| {
            !matches!(key.as_str(), "task" | "context" | "effort" | "model")
        }) {
            return Err(format!(
                "unknown argument of infer with tools: {key} (it answers \
                 with text: no schema)"
            )
            .into());
        }
        let mut blocks = texts(fields.remove("task"))
            .filter(|blocks| blocks.iter().any(|text| !text.trim().is_empty()))
            .ok_or("task must be a text, or a list of texts, not blank")?;
        match fields.remove("context") {
            None | Some(Value::Null) => {}
            Some(context) => {
                let last = blocks.last_mut().expect("a task has a text");
                last.push_str("\n\n");
                last.push_str(&context.to_string());
            }
        }
        let effort = match fields.remove("effort") {
            None | Some(Value::Null) => None,
            Some(effort) => Some(serde_json::from_value(effort).map_err(
                |_| "effort is none, minimal, low, medium, high, xhigh or max",
            )?),
        };
        let model = match fields.remove("model") {
            None | Some(Value::Null) => None,
            Some(Value::String(model)) if !model.trim().is_empty() => {
                Some(model)
            }
            Some(_) => {
                return Err("model must be a model's id or family".into());
            }
        };
        Ok(Self {
            blocks,
            effort,
            model,
            instructions,
            tools,
            turns,
        })
    }

    /// What the model is shown, stamped `now`: the task, then each round
    /// before it.
    pub(crate) fn messages(
        &self,
        now: tau_ai::message::Timestamp,
    ) -> Vec<tau_ai::message::Message> {
        use tau_ai::message::{Message, ToolResultMessage};
        let mut messages = vec![said(true, self.blocks.clone(), now)];
        for (message, results) in &self.turns {
            messages.push(Message::Assistant(message.clone()));
            for result in results {
                messages.push(Message::ToolResult(ToolResultMessage {
                    tool_call_id: result.id.clone(),
                    tool_name: result.name.clone(),
                    content: tau_agent::tool::ToolOutput::text(
                        result.text.clone(),
                    )
                    .content,
                    details: None,
                    is_error: !result.ok,
                    timestamp: now,
                }));
            }
        }
        messages
    }

    /// The instructions the round asks with.
    pub(crate) fn instructions(&self) -> String {
        self.instructions
            .clone()
            .unwrap_or_else(|| TOOL_INSTRUCTIONS.to_owned())
    }
}

/// What a round's `answer` says: `{ text, calls, message }`, each call
/// `{ id, name, args }`, and the answer itself to send back; refused when
/// it stopped with an error or at the model's output limit.
pub(crate) fn round_answer(
    answer: &AssistantMessage,
) -> Result<Value, Refused> {
    use tau_ai::message::StopReason;
    let error = answer.error_message.as_deref();
    match answer.stop_reason {
        StopReason::Error => {
            return Err(Refused {
                error: error.unwrap_or("Unknown error").to_owned(),
                reason: Some("error"),
            });
        }
        StopReason::Length => {
            return Err(Refused {
                error: "the answer hit the model's output limit and is \
                        incomplete"
                    .to_owned(),
                reason: Some("length"),
            });
        }
        _ => {}
    }
    if let Some(error) = error {
        return Err(Refused {
            error: error.to_owned(),
            reason: Some("error"),
        });
    }
    let calls: Vec<Value> = answer
        .tool_calls()
        .map(|call| {
            json!({ "id": call.id, "name": call.name, "args": call.arguments })
        })
        .collect();
    Ok(json!({
        "text": answer.text(),
        "calls": calls,
        "message": answer,
    }))
}
