//! A run's title, written by a model from the prompt it started with.
//!
//! One call, as Codex names its threads: the plan's newest Luna at low
//! effort, falling back to the run's own model, with the reply held to a
//! JSON object of one short `title`. Until it answers, and when it
//! cannot, the run shows the prompt's first line.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::Deserialize;
use serde_json::{Value, json};
use tau_ai::{
    event::AssistantEvent,
    llm::Llm,
    model::{find, plan_models},
    responses::request::{ReasoningEffort, Settings},
};

/// The longest title, in characters.
pub const MAX_CHARS: usize = 36;

/// The longest request, in bytes: the instructions and the prompt.
const PROMPT_MAX_BYTES: usize = 960;

/// The family titles are written with, when the plan offers it.
const FAMILY: &str = "luna";

/// What the model is asked to do.
fn instructions() -> String {
    format!(
        "Generate a concise, single-line task title of at most \
         {MAX_CHARS} characters and under five words where possible. \
         Start with an imperative verb. Capitalize only the first word \
         unless the user's language, proper nouns, acronyms, or code terms \
         require otherwise. Preserve ticket references exactly. Write in \
         the user's language. Do not use quotes, markdown, or trailing \
         punctuation. Do not answer the request."
    )
}

/// What a run on `prompt` is about: the goal's words for a `/goal`,
/// the prompt itself otherwise.
fn task(prompt: &str) -> String {
    tau_goal::set_message(prompt).unwrap_or_else(|| prompt.to_owned())
}

/// The request for `prompt`'s title, at most [`PROMPT_MAX_BYTES`]: the
/// prompt is cut short, never inside a character.
pub fn request(prompt: &str) -> String {
    let prefix = format!("{}\n\nUser prompt:\n", instructions());
    let room = PROMPT_MAX_BYTES.saturating_sub(prefix.len());
    let task = task(prompt);
    let task: String = task
        .trim()
        .char_indices()
        .take_while(|(at, c)| at + c.len_utf8() <= room)
        .map(|(_, c)| c)
        .collect();
    prefix + &task
}

/// The `text.format` the reply must follow: one nonempty `title` of at
/// most [`MAX_CHARS`].
pub fn format() -> Value {
    json!({
        "type": "json_schema",
        "name": "run_title",
        "schema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": MAX_CHARS,
                },
            },
            "required": ["title"],
            "additionalProperties": false,
        },
        "strict": true,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    title: String,
}

/// The title in a reply: without quotes around it, runs of whitespace,
/// or a final `.`, `?` or `!`, and at most [`MAX_CHARS`]. `None` when
/// the reply is not the object asked for or the title comes out empty.
pub fn parse(reply: &str) -> Option<String> {
    let Reply { title } = serde_json::from_str(reply.trim()).ok()?;
    let title = title
        .trim()
        .trim_matches(['"', '\'', '`', '“', '”', '‘', '’'])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let title = title.trim_end_matches(['.', '?', '!']).trim_end();
    (!title.is_empty()).then(|| title.chars().take(MAX_CHARS).collect())
}

/// What a run shows until its title is written: the first line of
/// what it is about, cut at [`MAX_CHARS`] with an ellipsis.
pub fn placeholder(prompt: &str) -> String {
    let task = task(prompt);
    let line = task
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if line.is_empty() {
        return "Untitled".into();
    }
    if line.chars().count() <= MAX_CHARS {
        return line;
    }
    let cut: String = line.chars().take(MAX_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The model titles are written with: the plan's newest Luna, or `run`'s
/// own model when the plan has none. Low effort when it takes one.
pub fn model(run: &str) -> (String, Option<ReasoningEffort>) {
    let model = plan_models()
        .into_iter()
        .find(|model| {
            tau_ai::model::family_version(&model.id)
                .is_some_and(|(family, _)| family == FAMILY)
        })
        .map_or_else(|| run.to_owned(), |model| model.id.clone());
    let effort = find(&model)
        .is_some_and(|model| model.efforts.contains(&ReasoningEffort::Low))
        .then_some(ReasoningEffort::Low);
    (model, effort)
}

/// Asks `llm` for the title of a run on `prompt` that runs on `run_model`.
pub async fn write(
    llm: &dyn Llm,
    run_model: &str,
    prompt: &str,
) -> anyhow::Result<String> {
    let (model, reasoning) = model(run_model);
    let settings = Settings {
        model,
        reasoning,
        text_format: Some(format()),
        ..Settings::default()
    };
    let mut session = llm.open(settings).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64);
    let transcript = [tau_ai::message::Message::User(
        tau_ai::message::UserMessage {
            content: tau_ai::message::UserContent::Text(request(prompt)),
            timestamp: now,
        },
    )];
    let mut events = session.respond(&transcript, now);
    let mut reply = String::new();
    while let Some(event) = futures_util::StreamExt::next(&mut events).await {
        match event {
            AssistantEvent::TextEnd { content, .. } => {
                reply.push_str(&content.text)
            }
            AssistantEvent::Error { message, .. } => anyhow::bail!(message),
            _ => {}
        }
    }
    parse(&reply).with_context(|| format!("not a title: {reply:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies_are_cleaned_up() {
        assert_eq!(
            parse(r#"{"title":"  \"Fix  the retry loop.\" "}"#).as_deref(),
            Some("Fix the retry loop")
        );
        assert_eq!(parse(r#"{"title":"Why?!"}"#).as_deref(), Some("Why"));
        assert_eq!(parse(r#"{"title":" ... "}"#), None);
        assert_eq!(parse(r#"{"title":"a","extra":1}"#), None);
        assert_eq!(parse("Fix the loop"), None);
        let long = format!(r#"{{"title":"{}"}}"#, "é".repeat(50));
        assert_eq!(parse(&long).unwrap().chars().count(), MAX_CHARS);
    }

    #[test]
    fn requests_fit_the_budget() {
        let long = "ü".repeat(2000);
        let request = request(&long);
        assert!(request.len() <= PROMPT_MAX_BYTES);
        assert!(request.ends_with('ü'));
        assert!(request.contains("User prompt:\nü"));
    }

    #[test]
    fn requests_carry_a_goal_not_the_command() {
        let request = request("/goal --continuations 3 all tests pass");
        assert!(request.ends_with("User prompt:\nall tests pass"));
    }

    #[test]
    fn placeholders_are_the_first_line() {
        assert_eq!(
            placeholder("\n  Fix the   retry loop\nin the client"),
            "Fix the retry loop"
        );
        assert_eq!(placeholder("  \n "), "Untitled");
        assert_eq!(
            placeholder("/goal --continuations 3 all tests pass"),
            "all tests pass"
        );
        let long = placeholder(&"word ".repeat(20));
        assert!(long.chars().count() <= MAX_CHARS, "{long}");
        assert!(long.ends_with('…'));
    }

    #[test]
    fn titles_are_written_by_the_newest_luna() {
        let (model, effort) = model("gpt-5.5");
        assert!(model.ends_with("-luna"), "{model}");
        assert_eq!(effort, Some(ReasoningEffort::Low));
    }
}
