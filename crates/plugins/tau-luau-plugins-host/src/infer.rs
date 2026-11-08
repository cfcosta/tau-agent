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
/// or in the run's `conversation`: the message goes as written, with no
/// bound but the VM's on what it holds, and the answer is its text.
pub(crate) struct Own {
    /// The task, then the context as JSON unless it is null.
    pub message: String,
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
                    | "effort"
                    | "model"
            )
        }) {
            return Err(format!(
                "unknown argument of a request of the plugin's own: {key}"
            )
            .into());
        }
        let mut message = match fields.remove("task") {
            Some(Value::String(task)) if !task.trim().is_empty() => task,
            _ => return Err("task must be a string, not blank".into()),
        };
        match fields.remove("context") {
            None | Some(Value::Null) => {}
            Some(context) => {
                message.push_str("\n\n");
                message.push_str(&context.to_string());
            }
        }
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
            && (instructions.is_some() || effort.is_some() || model.is_some())
        {
            return Err("in the run's conversation, `infer` asks with the \
                        run's own settings: no instructions, effort or model"
                .into());
        }
        if fields.contains_key("schema") {
            return Err("a request of the plugin's own takes no schema".into());
        }
        Ok(Self {
            message,
            instructions,
            conversation,
            effort,
            model,
        })
    }
}
