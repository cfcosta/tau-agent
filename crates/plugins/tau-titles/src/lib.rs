//! Run titles, as a plugin (ADR 0045): it fills the `tau.title` slot,
//! asking the person's model for a short title of what a run is about.
//!
//! One call, as Codex names its threads: the plan's newest Luna at low
//! effort, falling back to the run's own model, with the reply held to a
//! JSON object of one short `title`.

#[cfg(feature = "host")]
mod half;

#[cfg(feature = "host")]
pub use half::TitlesHost;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_ai::{
    model::{find, plan_models},
    responses::request::ReasoningEffort,
};
use tau_ui_plugin::{
    Declaration,
    Manifest,
    UiPlugin,
    ViewCx,
    services::names,
    title::{MAX_CHARS, TITLE},
    tree::Node,
};

pub const NAME: &str = "tau-titles";

/// The longest request, in bytes: the instructions and the prompt.
const PROMPT_MAX_BYTES: usize = 960;

/// The family titles are written with, when the plan offers it.
const FAMILY: &str = "luna";

/// What the person set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Whether titles are written; on until switched off.
    pub on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { on: true }
    }
}

/// tau-titles with its UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct TitlesUi;

impl UiPlugin for TitlesUi {
    type State = ();
    type Data = ();
    type RepoData = ();
    type Settings = Settings;

    fn name(&self) -> &'static str {
        NAME
    }

    fn declaration(&self) -> Declaration {
        Declaration::new()
            .fills(&[TITLE.name()])
            .services(&[names::MODEL])
    }

    fn manifest(&self) -> Manifest<Self> {
        Manifest::new().settings(pane)
    }

    fn on_action(&self, action: &str, _: &Value, view: &mut ViewCx<'_, Self>) {
        if action == SWITCH {
            let next = Settings {
                on: !view.settings.on,
            };
            view.save_settings(&next);
        }
    }
}

/// The pane's switch.
pub const SWITCH: &str = "switch";

/// Its settings pane: the switch.
fn pane(view: &mut ViewCx<'_, TitlesUi>) -> Node {
    Node::Card {
        children: vec![Node::Switch {
            label: "Title chats with a model".into(),
            detail: Some(
                "One short request as a chat starts, to the plan's smallest \
                 model. Off, a chat keeps the first line of its prompt."
                    .into(),
            ),
            on: view.settings.on,
            action: SWITCH.into(),
            args: Value::Null,
        }],
    }
}

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

/// The request for the title of a run about `task`, at most
/// `PROMPT_MAX_BYTES`: the task is cut short, never inside a character.
pub fn request(task: &str) -> String {
    let prefix = format!("{}\n\nUser prompt:\n", instructions());
    let room = PROMPT_MAX_BYTES.saturating_sub(prefix.len());
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

    /// Characters of one to four bytes, and white space.
    const UNITS: [char; 6] = ['a', 'Z', 'é', '語', '🦀', ' '];

    /// A request is the instructions, then the most of the trimmed task
    /// that fits in `PROMPT_MAX_BYTES`, cut between characters. Tasks
    /// are drawn with lengths around the room left, so the last
    /// character fits exactly, or one byte short, or past it.
    #[hegel::test(test_cases = 300)]
    fn requests_fit_the_budget(tc: hegel::TestCase) {
        use hegel::generators as gs;
        let prefix = request("");
        assert!(prefix.ends_with("User prompt:\n"));
        let room = PROMPT_MAX_BYTES - prefix.len();
        let target: usize = tc.draw(hegel::one_of!(
            gs::integers::<usize>()
                .min_value(room - 4)
                .max_value(room + 4),
            gs::integers::<usize>().max_value(room + 40),
        ));
        let mut task = String::new();
        while task.len() < target {
            task.push(tc.draw(gs::sampled_from(UNITS.to_vec())));
        }
        let trimmed = task.trim();
        // The longest prefix of whole characters within the room.
        let kept = trimmed
            .char_indices()
            .map(|(at, c)| at + c.len_utf8())
            .take_while(|end| *end <= room)
            .last()
            .unwrap_or(0);
        assert_eq!(request(&task), format!("{prefix}{}", &trimmed[..kept]));
    }

    #[test]
    fn titles_are_written_by_the_newest_luna() {
        let (model, effort) = model("gpt-5.5");
        assert!(model.ends_with("-luna"), "{model}");
        assert_eq!(effort, Some(ReasoningEffort::Low));
    }
}
