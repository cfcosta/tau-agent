//! A constitution: rules, where each applies, and how sure Jev must be
//! that one is broken to review or block. Read from TOML:
//!
//! ```toml
//! # What to do when Jev cannot answer: "allow" (the default) or "block".
//! on_error = "allow"
//! # How many times one run's final answer may be sent back.
//! max_holds = 3
//!
//! [[rule]]
//! id = "R2"
//! text = "Library code returns errors. No unwrap or expect outside tests."
//! on = ["edit.newText", "write.content"]
//! review = 0.3   # flag for a person at this violation probability...
//! block = 0.8    # ...and refuse the call at this one
//!
//! [[rule]]
//! id = "R6"
//! text = "The final answer names the tests that ran and their result."
//! on = ["final answer"]
//! ```

use std::path::Path;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Where a rule applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// An argument of a tool: `edit.newText` is every `newText` in an
    /// `edit` call's arguments, however deep.
    Field { tool: String, field: String },
    /// The final answer, when the run would stop.
    FinalAnswer,
}

impl Target {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("final answer") {
            return Ok(Self::FinalAnswer);
        }
        match text.split_once('.') {
            Some((tool, field)) if !tool.is_empty() && !field.is_empty() => {
                Ok(Self::Field {
                    tool: tool.to_owned(),
                    field: field.to_owned(),
                })
            }
            _ => bail!(
                "`{text}` names no target: write `tool.field` (such as \
                 `edit.newText`) or `final answer`"
            ),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Field { tool, field } => format!("{tool}.{field}"),
            Self::FinalAnswer => "final answer".into(),
        }
    }
}

/// One rule.
#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub id: String,
    pub text: String,
    pub on: Vec<Target>,
    /// At this violation probability, the call runs but is flagged.
    pub review: f64,
    /// At this one, the call is refused (or the stop held).
    pub block: f64,
}

/// What to do when Jev gives no answer.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum OnError {
    /// Let the call run, and report the failure.
    #[default]
    Allow,
    /// Refuse the call: nothing unchecked runs.
    Block,
}

/// The rules one agent (or repository) keeps.
#[derive(Debug, Clone, PartialEq)]
pub struct Constitution {
    pub rules: Vec<Rule>,
    pub on_error: OnError,
    /// How many times a run's final answer may be sent back.
    pub max_holds: u32,
}

impl Default for Constitution {
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            on_error: OnError::Allow,
            max_holds: DEFAULT_MAX_HOLDS,
        }
    }
}

pub const DEFAULT_MAX_HOLDS: u32 = 3;
pub const DEFAULT_REVIEW: f64 = 0.5;
pub const DEFAULT_BLOCK: f64 = 0.8;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    on_error: OnError,
    max_holds: Option<u32>,
    #[serde(default, rename = "rule")]
    rules: Vec<RuleFile>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuleFile {
    id: String,
    text: String,
    on: Vec<String>,
    review: Option<f64>,
    block: Option<f64>,
}

impl Constitution {
    /// Parses a constitution, checking every rule.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let file: File = toml::from_str(text)?;
        let mut rules: Vec<Rule> = Vec::with_capacity(file.rules.len());
        for rule in file.rules {
            let rule = checked(rule)?;
            if rules.iter().any(|known| known.id == rule.id) {
                bail!("Two rules are called {}", rule.id);
            }
            rules.push(rule);
        }
        Ok(Self {
            rules,
            on_error: file.on_error,
            max_holds: file.max_holds.unwrap_or(DEFAULT_MAX_HOLDS),
        })
    }

    /// The constitution as TOML, for [`Self::parse`] to read back.
    pub fn to_toml(&self) -> String {
        let file = File {
            on_error: self.on_error,
            max_holds: Some(self.max_holds),
            rules: self
                .rules
                .iter()
                .map(|rule| RuleFile {
                    id: rule.id.clone(),
                    text: rule.text.clone(),
                    on: rule.on.iter().map(Target::label).collect(),
                    review: Some(rule.review),
                    block: Some(rule.block),
                })
                .collect(),
        };
        toml::to_string_pretty(&file).expect("a constitution serializes")
    }

    /// Writes the constitution to `path`, making its directory.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, self.to_toml())
            .with_context(|| format!("Cannot write {}", path.display()))
    }

    /// Adds a rule, checked like one read from a file, under the next
    /// free id (`R1`, `R2`…). Returns its id.
    pub fn add(
        &mut self,
        text: &str,
        on: &[String],
        review: f64,
        block: f64,
    ) -> anyhow::Result<String> {
        let id = (1..)
            .map(|n| format!("R{n}"))
            .find(|id| self.rules.iter().all(|rule| &rule.id != id))
            .expect("some id is free");
        self.rules.push(checked(RuleFile {
            id: id.clone(),
            text: text.to_owned(),
            on: on.to_vec(),
            review: Some(review),
            block: Some(block),
        })?);
        Ok(id)
    }

    /// Removes the rule `id`. Returns whether there was one.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|rule| rule.id != id);
        self.rules.len() != before
    }

    /// Reads the constitution at `path`; none there is no rules.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("{} is not valid", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::default())
            }
            Err(error) => Err(error)
                .with_context(|| format!("Cannot read {}", path.display())),
        }
    }

    /// The rules for a call to `tool`, each with the arguments it is
    /// about: the values of its fields, and nothing else.
    pub fn for_call<'a>(
        &'a self,
        tool: &str,
        args: &Value,
    ) -> Vec<(&'a Rule, Map<String, Value>)> {
        self.rules
            .iter()
            .filter_map(|rule| {
                let mut fields = Map::new();
                for target in &rule.on {
                    if let Target::Field { tool: t, field } = target
                        && t == tool
                    {
                        let mut found = Vec::new();
                        collect(args, field, &mut found);
                        match found.len() {
                            0 => {}
                            1 => {
                                fields.insert(field.clone(), found.remove(0));
                            }
                            _ => {
                                fields
                                    .insert(field.clone(), Value::Array(found));
                            }
                        }
                    }
                }
                (!fields.is_empty()).then_some((rule, fields))
            })
            .collect()
    }

    /// The rules for the final answer.
    pub fn for_final_answer(&self) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| rule.on.contains(&Target::FinalAnswer))
            .collect()
    }
}

/// A rule as written, checked: an id, text, somewhere to apply, and
/// thresholds that are probabilities with review at most block.
fn checked(rule: RuleFile) -> anyhow::Result<Rule> {
    let id = rule.id.trim().to_owned();
    if id.is_empty() {
        bail!("A rule has no id");
    }
    if rule.text.trim().is_empty() {
        bail!("Rule {id} has no text");
    }
    if rule.on.is_empty() {
        bail!("Rule {id} applies nowhere: give it `on`");
    }
    let on = rule
        .on
        .iter()
        .map(|target| Target::parse(target))
        .collect::<anyhow::Result<Vec<_>>>()
        .with_context(|| format!("Rule {id}"))?;
    let review = rule.review.unwrap_or(DEFAULT_REVIEW);
    let block = rule.block.unwrap_or(DEFAULT_BLOCK.max(review));
    if !(0.0..=1.0).contains(&review) || !(0.0..=1.0).contains(&block) {
        bail!("Rule {id}: review and block are probabilities, 0 to 1");
    }
    if review > block {
        bail!("Rule {id}: review ({review}) is above block ({block})");
    }
    Ok(Rule {
        id,
        text: rule.text.trim().to_owned(),
        on,
        review,
        block,
    })
}

/// Every value under `key` in `value`, however deep.
fn collect(value: &Value, key: &str, found: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            for (name, inner) in map {
                if name == key {
                    found.push(inner.clone());
                } else {
                    collect(inner, key, found);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, key, found);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const EXAMPLE: &str = r#"
        max_holds = 2
        [[rule]]
        id = "R2"
        text = "No unwrap outside tests."
        on = ["edit.newText", "write.content"]
        review = 0.3
        block = 0.8

        [[rule]]
        id = "R6"
        text = "Name the tests that ran."
        on = ["final answer"]
    "#;

    #[test]
    fn a_constitution_reads_from_toml() {
        let constitution = Constitution::parse(EXAMPLE).unwrap();
        assert_eq!(constitution.max_holds, 2);
        assert_eq!(constitution.on_error, OnError::Allow);
        let r2 = &constitution.rules[0];
        assert_eq!(r2.on[0].label(), "edit.newText");
        assert_eq!((r2.review, r2.block), (0.3, 0.8));
        let r6 = &constitution.rules[1];
        assert_eq!(r6.on, [Target::FinalAnswer]);
        assert_eq!((r6.review, r6.block), (DEFAULT_REVIEW, DEFAULT_BLOCK));
    }

    #[test]
    fn bad_constitutions_say_what_is_wrong() {
        let bad = |text: &str| {
            format!("{:#}", Constitution::parse(text).unwrap_err())
        };
        assert!(
            bad("[[rule]]\nid='A'\ntext='t'\non=[]")
                .contains("applies nowhere")
        );
        assert!(
            bad("[[rule]]\nid='A'\ntext='t'\non=['bash']")
                .contains("names no target")
        );
        assert!(
            bad(
                "[[rule]]\nid='A'\ntext='t'\non=['a.b']\nreview=0.9\nblock=0.5"
            )
            .contains("above block")
        );
        let twice = "[[rule]]\nid='A'\ntext='t'\non=['a.b']\n[[rule]]\nid='A'\ntext='u'\non=['a.b']";
        assert!(bad(twice).contains("Two rules"));
        assert!(bad("unknown = 1").contains("unknown"));
    }

    #[test]
    fn only_the_fields_a_rule_names_are_checked() {
        let constitution = Constitution::parse(EXAMPLE).unwrap();
        let args = json!({
            "path": "src/lib.rs",
            "edits": [
                {"oldText": "a", "newText": "x.unwrap()"},
                {"oldText": "b", "newText": "y"}
            ]
        });
        let found = constitution.for_call("edit", &args);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.id, "R2");
        assert_eq!(
            found[0].1,
            json!({"newText": ["x.unwrap()", "y"]})
                .as_object()
                .unwrap()
                .clone()
        );
        assert!(
            constitution
                .for_call("bash", &json!({"command": "ls"}))
                .is_empty()
        );
        assert!(
            constitution
                .for_call("edit", &json!({"path": "a"}))
                .is_empty()
        );
        assert_eq!(constitution.for_final_answer().len(), 1);
    }

    #[test]
    fn rules_added_and_removed_round_trip() {
        let mut constitution = Constitution::parse(EXAMPLE).unwrap();
        let id = constitution
            .add("Never force-push.", &["bash.command".into()], 0.2, 0.6)
            .unwrap();
        assert_eq!(id, "R1");
        assert!(constitution.add("x", &[], 0.5, 0.8).is_err());
        assert!(constitution.add("x", &["bash".into()], 0.5, 0.8).is_err());
        assert!(constitution.add("x", &["a.b".into()], 0.9, 0.1).is_err());
        let again = Constitution::parse(&constitution.to_toml()).unwrap();
        assert_eq!(again, constitution);
        assert!(constitution.remove("R2"));
        assert!(!constitution.remove("R2"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a/constitution.toml");
        constitution.save(&path).unwrap();
        assert_eq!(Constitution::load(&path).unwrap(), constitution);
    }

    #[test]
    fn a_missing_file_is_no_rules() {
        let dir = tempfile::tempdir().unwrap();
        let constitution =
            Constitution::load(&dir.path().join("none.toml")).unwrap();
        assert!(constitution.rules.is_empty());
    }
}
