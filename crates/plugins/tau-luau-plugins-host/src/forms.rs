//! A form a run's tool asks the person (ADR 0050, gap C): one to four
//! questions, each with its choices and, if it says so, the person's own
//! answer. The tool answers `tau.ask{ questions, action }` and its call
//! waits, as one that answers `tau.allow` does; the plugin's view draws
//! the form, and one of its actions gives the answers
//! (`tau.answer{ call, answers }`), from whichever device answers
//! first. The host holds the answers to what was asked, then calls
//! `actions[action]` in the run with them, and the model reads what it
//! answers.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The most questions one form asks.
pub const MAX_QUESTIONS: usize = 4;

/// What a tool asks: `tau.ask`'s answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Form {
    pub questions: Vec<Question>,
    /// The plugin's action called in the run once the person answered,
    /// with `{ call, answers }` or `{ call, declined }`.
    #[serde(default)]
    pub action: Option<String>,
}

/// One question of a form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    #[serde(default)]
    pub header: String,
    #[serde(default, deserialize_with = "list")]
    pub options: Vec<Choice>,
    /// Whether several choices may be picked.
    #[serde(default)]
    pub multi_select: bool,
    /// Whether the person may write their own answer.
    #[serde(default)]
    pub other: bool,
}

/// One choice of a question.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: Option<String>,
}

/// The person's answers to a form, or that they would not answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Reply {
    Declined { declined: bool },
    Answered { answers: Vec<Answer> },
}

/// The answer to one question.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The labels picked, in the order the question offers them.
    #[serde(default, deserialize_with = "list")]
    pub picked: Vec<String>,
    /// What the person wrote, when the question takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
    /// What they added about the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A list, which an empty Luau table may also send as an object.
fn list<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    match Value::deserialize(deserializer)? {
        Value::Null => Ok(Vec::new()),
        Value::Object(map) if map.is_empty() => Ok(Vec::new()),
        other => {
            serde_json::from_value(other).map_err(serde::de::Error::custom)
        }
    }
}

/// What `value`, a tool's answer, asks the person: its `tau_ask`.
pub fn asked(value: &Value) -> Option<Result<Form, String>> {
    let asked = value.get("tau_ask")?;
    Some(
        serde_json::from_value::<Form>(asked.clone())
            .map_err(|error| format!("`tau_ask` is not a form: {error}"))
            .and_then(|form| form.check().map(|()| form)),
    )
}

/// What an action's answer gives a waiting call: its `tau_answer`, the
/// call and the reply.
pub fn answered(value: &Value) -> Option<Result<(String, Reply), String>> {
    let answer = value.get("tau_answer")?;
    let call = answer["call"].as_str().unwrap_or_default().to_owned();
    if answer["declined"].as_bool() == Some(true) {
        return Some(Ok((call, Reply::Declined { declined: true })));
    }
    Some(
        serde_json::from_value::<Vec<Answer>>(answer["answers"].clone())
            .map(|answers| (call, Reply::Answered { answers }))
            .map_err(|error| format!("`tau_answer` holds no answers: {error}")),
    )
}

impl Form {
    /// Why the form cannot be asked, if it cannot: one to
    /// [`MAX_QUESTIONS`] questions, each with its text and at least one
    /// choice, its labels written and each once.
    pub fn check(&self) -> Result<(), String> {
        let n = self.questions.len();
        if !(1..=MAX_QUESTIONS).contains(&n) {
            return Err(format!(
                "A form asks 1 to {MAX_QUESTIONS} questions, not {n}."
            ));
        }
        for (i, question) in self.questions.iter().enumerate() {
            let at = format!("Question {}", i + 1);
            if question.question.trim().is_empty() {
                return Err(format!("{at} has no text."));
            }
            if question.options.is_empty() {
                return Err(format!("{at} offers no choice."));
            }
            for (k, choice) in question.options.iter().enumerate() {
                let label = choice.label.trim();
                if label.is_empty() {
                    return Err(format!(
                        "{at}'s choice {} has no label.",
                        k + 1
                    ));
                }
                if question.options[..k]
                    .iter()
                    .any(|before| before.label.trim() == label)
                {
                    return Err(format!("{at} offers \"{label}\" twice."));
                }
            }
        }
        Ok(())
    }

    /// Why `reply` does not answer the form, if it does not: an answer
    /// per question, each picking the question's own choices in the
    /// order it offers them, one where one is picked, the person's own
    /// only where the question takes it, and none empty.
    pub fn fits(&self, reply: &Reply) -> Result<(), String> {
        let Reply::Answered { answers } = reply else {
            return Ok(());
        };
        if answers.len() != self.questions.len() {
            return Err(format!(
                "{} answers for {} questions.",
                answers.len(),
                self.questions.len()
            ));
        }
        for (question, answer) in self.questions.iter().zip(answers) {
            let own = answer
                .other
                .as_deref()
                .map(str::trim)
                .filter(|own| !own.is_empty());
            if own.is_some() && !question.other {
                return Err(format!(
                    "\"{}\" takes no answer of the person's own.",
                    question.header
                ));
            }
            let given = answer.picked.len() + usize::from(own.is_some());
            if given == 0 {
                return Err(format!("\"{}\" has no answer.", question.header));
            }
            if !question.multi_select && given > 1 {
                return Err(format!(
                    "\"{}\" takes one answer, not {given}.",
                    question.header
                ));
            }
            let labels: Vec<&str> =
                question.options.iter().map(|c| c.label.as_str()).collect();
            if let Some(stray) =
                answer.picked.iter().find(|p| !labels.contains(&p.as_str()))
            {
                return Err(format!(
                    "\"{}\" offers no choice \"{stray}\".",
                    question.header
                ));
            }
            let at: Vec<usize> = answer
                .picked
                .iter()
                .filter_map(|p| labels.iter().position(|l| l == p))
                .collect();
            if at.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(format!(
                    "\"{}\" picks its choices more than once or out of order.",
                    question.header
                ));
            }
        }
        Ok(())
    }
}

/// A form waiting: whose it is, and what it asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Asked {
    pub plugin: String,
    pub form: Form,
}

/// The forms waiting for the person, by run and call: the first answer
/// that fits takes one.
pub type Forms = crate::chats::Waits<Reply, Asked>;

/// Gives the call `call` of `run`, waiting on a form of `plugin`'s, the
/// person's `reply`, when it fits what was asked.
pub fn answer(
    forms: &Forms,
    plugin: &str,
    run: &str,
    call: &str,
    reply: Reply,
) -> anyhow::Result<()> {
    let checked = reply.clone();
    forms.answer_if(run, call, reply, |asked| {
        if asked.plugin != plugin {
            return Err(format!("{plugin} did not ask that"));
        }
        asked.form.fits(&checked)
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn form() -> Form {
        serde_json::from_value(json!({ "questions": [
            { "question": "Which?", "header": "Pick", "options": [
                { "label": "A" }, { "label": "B" } ] },
            { "question": "Which ones?", "header": "Many", "multi_select": true,
              "other": true, "options": [ { "label": "X" }, { "label": "Y" } ] },
        ]}))
        .unwrap()
    }

    fn answers(given: Value) -> Reply {
        Reply::Answered {
            answers: serde_json::from_value(given).unwrap(),
        }
    }

    /// A form asks one to four questions, each with distinct, written
    /// choices.
    #[test]
    fn a_form_is_checked_before_it_waits() {
        assert_eq!(form().check(), Ok(()));
        let mut wrong = form();
        wrong.questions.clear();
        assert!(wrong.check().unwrap_err().contains("1 to 4"));
        let mut wrong = form();
        wrong.questions[1].options[1].label = " X ".into();
        assert!(wrong.check().unwrap_err().starts_with("Question 2"));
        let mut wrong = form();
        wrong.questions[0].options.clear();
        assert!(wrong.check().unwrap_err().starts_with("Question 1"));
        let asked = asked(&json!({ "tau_ask": { "questions": {} } }));
        assert!(asked.unwrap().is_err(), "an empty table asks nothing");
    }

    /// Answers fit when each question has its own, in order, one where
    /// one is picked, and the person's own only where it is taken.
    #[test]
    fn answers_fit_the_form_or_are_refused() {
        let form = form();
        let fits = |given| form.fits(&answers(given));
        assert_eq!(
            fits(
                json!([{ "picked": ["B"] }, { "picked": ["X", "Y"], "other": "z" }])
            ),
            Ok(())
        );
        assert!(fits(json!([{ "picked": ["B"] }])).is_err());
        assert!(
            fits(json!([{ "picked": ["A", "B"] }, { "picked": ["X"] }]))
                .is_err()
        );
        assert!(
            fits(json!([{ "other": "mine" }, { "picked": ["X"] }])).is_err()
        );
        assert!(
            fits(json!([{ "picked": ["B"] }, { "picked": ["Y", "X"] }]))
                .is_err()
        );
        assert!(
            fits(json!([{ "picked": ["C"] }, { "picked": ["X"] }])).is_err()
        );
        assert!(fits(json!([{ "picked": [] }, { "other": "  " }])).is_err());
        assert_eq!(form.fits(&Reply::Declined { declined: true }), Ok(()));
    }

    /// The first answer that fits takes the call, from whichever device;
    /// one that does not fit, or of another plugin, leaves it waiting.
    #[test]
    fn the_first_fitting_answer_takes_the_call() {
        let forms = Forms::default();
        let waiting = forms.wait(
            "r1",
            "c1",
            Asked {
                plugin: "p".into(),
                form: form(),
            },
        );
        let good = answers(json!([{ "picked": ["A"] }, { "picked": ["X"] }]));
        assert!(answer(&forms, "q", "r1", "c1", good.clone()).is_err());
        assert!(answer(&forms, "p", "r1", "c1", answers(json!([]))).is_err());
        assert!(forms.waits("r1", "c1"));
        answer(&forms, "p", "r1", "c1", good.clone()).unwrap();
        assert!(answer(&forms, "p", "r1", "c1", good.clone()).is_err());
        let got = tau_testing::block_on(waiting.answer());
        assert_eq!(got, Some(good));
        assert!(forms.waiting().is_empty());
    }
}
