//! The questions the agent asks, the answers it gets, and the text it
//! reads back.

use serde::{Deserialize, Serialize};

/// The most questions one call asks.
pub const MAX_QUESTIONS: usize = 4;
/// The fewest and most choices a question offers, besides the person's
/// own answer.
pub const MIN_CHOICES: usize = 2;
pub const MAX_CHOICES: usize = 4;
/// The longest a question's header is, in characters: it is a tab.
pub const MAX_HEADER: usize = 12;
/// What the person's own answer is called: no choice may take it.
pub const OTHER: &str = "Other";

/// What the agent asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "host", derive(schemars::JsonSchema))]
pub struct Ask {
    /// One to four questions, asked together.
    #[cfg_attr(feature = "host", schemars(length(min = 1, max = 4)))]
    pub questions: Vec<Question>,
}

/// One question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "host", derive(schemars::JsonSchema))]
pub struct Question {
    /// The whole question, ending in a question mark.
    pub question: String,
    /// A word or two for its tab, at most 12 characters: "Auth method".
    #[cfg_attr(feature = "host", schemars(length(min = 1, max = 12)))]
    pub header: String,
    /// Two to four choices. The person can always write their own
    /// answer instead, so do not offer an "Other". Put a recommended
    /// choice first and end its label with "(Recommended)".
    #[cfg_attr(feature = "host", schemars(length(min = 2, max = 4)))]
    pub options: Vec<Choice>,
    /// Whether several choices can be picked.
    #[serde(default)]
    pub multi_select: bool,
}

/// One choice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "host", derive(schemars::JsonSchema))]
pub struct Choice {
    /// One to five words.
    pub label: String,
    /// What picking it means, or what follows from it.
    pub description: String,
    /// Something to look at while the choice is in focus: a code
    /// snippet, a mockup, a diagram. Markdown, shown in a monospace box.
    /// Only for questions where one choice is picked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// How the person answered a call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Reply {
    /// An answer to each question, in order.
    Answered { answers: Vec<Answer> },
    /// The person would not answer.
    Declined,
}

/// The answer to one question.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// The labels picked, in the order the question offers them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub picked: Vec<String>,
    /// What the person wrote instead of, or besides, a choice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub other: Option<String>,
    /// What the person added about the answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Ask {
    /// Why the questions cannot be asked, if they cannot: the model reads
    /// it and asks again.
    pub fn check(&self) -> Result<(), String> {
        let n = self.questions.len();
        if !(1..=MAX_QUESTIONS).contains(&n) {
            return Err(format!(
                "Ask 1 to {MAX_QUESTIONS} questions at a time, not {n}."
            ));
        }
        for (i, question) in self.questions.iter().enumerate() {
            let at = format!("Question {}", i + 1);
            if question.question.trim().is_empty() {
                return Err(format!("{at} has no text."));
            }
            if self.questions[..i].iter().any(|before| {
                before.question.trim() == question.question.trim()
            }) {
                return Err(format!("{at} repeats an earlier question."));
            }
            let header = question.header.trim();
            if header.is_empty() || header.chars().count() > MAX_HEADER {
                return Err(format!(
                    "{at}'s header must be 1 to {MAX_HEADER} characters, not {:?}.",
                    question.header
                ));
            }
            let choices = question.options.len();
            if !(MIN_CHOICES..=MAX_CHOICES).contains(&choices) {
                return Err(format!(
                    "{at} must offer {MIN_CHOICES} to {MAX_CHOICES} choices, not \
                     {choices}."
                ));
            }
            for (k, choice) in question.options.iter().enumerate() {
                let label = choice.label.trim();
                if label.is_empty() {
                    return Err(format!(
                        "{at}'s choice {} has no label.",
                        k + 1
                    ));
                }
                if label.eq_ignore_ascii_case(OTHER) {
                    return Err(format!(
                        "{at} offers \"{OTHER}\": the person can always write their own \
                         answer, so leave it out."
                    ));
                }
                if question.options[..k]
                    .iter()
                    .any(|before| before.label.trim() == label)
                {
                    return Err(format!("{at} offers \"{label}\" twice."));
                }
                if question.multi_select && choice.preview.is_some() {
                    return Err(format!(
                        "{at} picks several choices, so its choices cannot have \
                         previews."
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Answer {
    /// The answer in a line: the labels picked, then what the person
    /// wrote, joined by commas.
    pub fn text(&self) -> String {
        self.picked
            .iter()
            .map(String::as_str)
            .chain(self.other())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What the person wrote, trimmed; none when it is blank.
    pub fn other(&self) -> Option<&str> {
        self.other
            .as_deref()
            .map(str::trim)
            .filter(|o| !o.is_empty())
    }
}

impl Reply {
    /// Why the reply does not answer `ask`, if it does not: an answer per
    /// question, each picking the question's own choices, one for a
    /// question where one is picked, and none empty.
    pub fn check(&self, ask: &Ask) -> Result<(), String> {
        let Self::Answered { answers } = self else {
            return Ok(());
        };
        if answers.len() != ask.questions.len() {
            return Err(format!(
                "{} answers for {} questions.",
                answers.len(),
                ask.questions.len()
            ));
        }
        for (question, answer) in ask.questions.iter().zip(answers) {
            let given =
                answer.picked.len() + usize::from(answer.other().is_some());
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
            // Each label once, in the order the question offers them.
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

    /// What the model reads back for `ask`.
    pub fn text(&self, ask: &Ask) -> String {
        let Self::Answered { answers } = self else {
            return "The person declined to answer. Go on with your best judgment, \
                    or ask in plain words if you cannot."
                .to_owned();
        };
        let mut text = String::from("The person answered:");
        for (question, answer) in ask.questions.iter().zip(answers) {
            text.push_str(&format!(
                "\n\"{}\" = \"{}\"",
                question.question.trim(),
                answer.text()
            ));
            if let Some(note) = answer.note.as_deref().map(str::trim)
                && !note.is_empty()
            {
                text.push_str("\n  note: ");
                text.push_str(&note.replace('\n', "\n        "));
            }
        }
        text
    }
}
