//! A constitution: rules, where each applies, and how sure Jev must be
//! that one is broken to review or block. It lives in the plugin's own
//! database, one per repository (`tau_constitution_host::db`), and is
//! edited only through tau's UI. Every rule is checked when it is added,
//! replaced or read back.
//!
//! A rule applies to tool arguments (`edit.newText` is every `newText` in
//! an `edit` call's arguments) or to the run's final answer:
//!
//! - `R2`: "Library code returns errors. No unwrap or expect outside
//!   tests.", on `edit.newText` and `write.content`, reviewed at a
//!   violation probability of 0.3 and blocked at 0.8.
//! - `R6`: "The final answer names the tests that ran and their result.",
//!   on the final answer.

use serde_json::{Map, Value};

/// A rule, or part of one, that does not check out. The UI shows the
/// message as it is.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RuleError {
    #[error(
        "`{0}` names no target: write `tool.field` (such as `edit.newText`) \
         or `final answer`"
    )]
    NoTarget(String),
    #[error("`{0}` is not allow or block")]
    NotOnError(String),
    #[error("A rule has no id")]
    NoId,
    #[error("Rule {0} has no text")]
    NoText(String),
    #[error("Rule {0} applies nowhere: give it somewhere to apply")]
    Nowhere(String),
    #[error("Rule {id}: {target}")]
    Target {
        id: String,
        #[source]
        target: Box<RuleError>,
    },
    #[error("Rule {0}: review and block are probabilities, 0 to 1")]
    NotProbabilities(String),
    #[error("Rule {id}: review ({review}) is above block ({block})")]
    ReviewAboveBlock { id: String, review: f64, block: f64 },
    #[error("Two rules are called {0}")]
    Duplicate(String),
    #[error("There is no rule {0}")]
    Unknown(String),
}

/// A repository's constitution as the database keeps it: plain values,
/// checked when [`Constitution`] reads them.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredConstitution {
    /// `allow` or `block`.
    pub on_error: String,
    pub max_holds: u32,
    /// In order.
    pub rules: Vec<StoredRule>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredRule {
    pub id: String,
    pub text: String,
    /// Where it applies: `tool.field`, or `final answer`.
    pub targets: Vec<String>,
    pub review: f64,
    pub block: f64,
}

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
    pub fn parse(text: &str) -> Result<Self, RuleError> {
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
            _ => Err(RuleError::NoTarget(text.to_owned())),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnError {
    /// Let the call run, and report the failure.
    #[default]
    Allow,
    /// Refuse the call: nothing unchecked runs.
    Block,
}

impl OnError {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block => "block",
        }
    }

    pub fn parse(text: &str) -> Result<Self, RuleError> {
        match text {
            "allow" => Ok(Self::Allow),
            "block" => Ok(Self::Block),
            other => Err(RuleError::NotOnError(other.to_owned())),
        }
    }
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

impl Constitution {
    /// A constitution read back from the database, every rule checked.
    pub fn from_stored(stored: StoredConstitution) -> Result<Self, RuleError> {
        let mut rules: Vec<Rule> = Vec::with_capacity(stored.rules.len());
        for rule in stored.rules {
            let rule = checked(
                &rule.id,
                &rule.text,
                &rule.targets,
                rule.review,
                rule.block,
            )?;
            if rules.iter().any(|known| known.id == rule.id) {
                return Err(RuleError::Duplicate(rule.id));
            }
            rules.push(rule);
        }
        Ok(Self {
            rules,
            on_error: OnError::parse(&stored.on_error)?,
            max_holds: stored.max_holds,
        })
    }

    /// The constitution as the database keeps it.
    pub fn to_stored(&self) -> StoredConstitution {
        StoredConstitution {
            on_error: self.on_error.as_str().to_owned(),
            max_holds: self.max_holds,
            rules: self
                .rules
                .iter()
                .map(|rule| StoredRule {
                    id: rule.id.clone(),
                    text: rule.text.clone(),
                    targets: rule.on.iter().map(Target::label).collect(),
                    review: rule.review,
                    block: rule.block,
                })
                .collect(),
        }
    }

    /// Adds a rule, checked, under the next free id (`R1`, `R2`…).
    /// Returns its id.
    pub fn add(
        &mut self,
        text: &str,
        on: &[String],
        review: f64,
        block: f64,
    ) -> Result<String, RuleError> {
        let id = (1..)
            .map(|n| format!("R{n}"))
            .find(|id| self.rules.iter().all(|rule| &rule.id != id))
            .expect("some id is free");
        self.rules.push(checked(&id, text, on, review, block)?);
        Ok(id)
    }

    /// Rewrites the rule `id` in place: same id, same position, checked.
    pub fn replace(
        &mut self,
        id: &str,
        text: &str,
        on: &[String],
        review: f64,
        block: f64,
    ) -> Result<(), RuleError> {
        let at = self
            .rules
            .iter()
            .position(|rule| rule.id == id)
            .ok_or_else(|| RuleError::Unknown(id.to_owned()))?;
        self.rules[at] = checked(id, text, on, review, block)?;
        Ok(())
    }

    /// Removes the rule `id`. Returns whether there was one.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.rules.len();
        self.rules.retain(|rule| rule.id != id);
        self.rules.len() != before
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

/// A rule, checked: an id, text, somewhere to apply, and thresholds
/// that are probabilities with review at most block.
fn checked(
    id: &str,
    text: &str,
    on: &[String],
    review: f64,
    block: f64,
) -> Result<Rule, RuleError> {
    let id = id.trim().to_owned();
    if id.is_empty() {
        return Err(RuleError::NoId);
    }
    if text.trim().is_empty() {
        return Err(RuleError::NoText(id));
    }
    if on.is_empty() {
        return Err(RuleError::Nowhere(id));
    }
    let on = match on
        .iter()
        .map(|target| Target::parse(target))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(on) => on,
        Err(target) => {
            return Err(RuleError::Target {
                id,
                target: Box::new(target),
            });
        }
    };
    if !(0.0..=1.0).contains(&review) || !(0.0..=1.0).contains(&block) {
        return Err(RuleError::NotProbabilities(id));
    }
    if review > block {
        return Err(RuleError::ReviewAboveBlock { id, review, block });
    }
    Ok(Rule {
        id,
        text: text.trim().to_owned(),
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

    fn rule(
        id: &str,
        text: &str,
        on: &[&str],
        review: f64,
        block: f64,
    ) -> StoredRule {
        StoredRule {
            id: id.into(),
            text: text.into(),
            targets: on.iter().map(|target| (*target).to_owned()).collect(),
            review,
            block,
        }
    }

    fn example() -> Constitution {
        Constitution::from_stored(StoredConstitution {
            on_error: "allow".into(),
            max_holds: 2,
            rules: vec![
                rule(
                    "R2",
                    "No unwrap outside tests.",
                    &["edit.newText", "write.content"],
                    0.3,
                    0.8,
                ),
                rule(
                    "R6",
                    "Name the tests that ran.",
                    &["final answer"],
                    0.5,
                    0.8,
                ),
            ],
        })
        .unwrap()
    }

    #[test]
    fn a_constitution_reads_back_from_the_store_shape() {
        let constitution = example();
        assert_eq!(constitution.max_holds, 2);
        assert_eq!(constitution.on_error, OnError::Allow);
        let r2 = &constitution.rules[0];
        assert_eq!(r2.on[0].label(), "edit.newText");
        assert_eq!((r2.review, r2.block), (0.3, 0.8));
        assert_eq!(constitution.rules[1].on, [Target::FinalAnswer]);
        assert_eq!(
            Constitution::from_stored(constitution.to_stored()).unwrap(),
            constitution
        );
    }

    #[test]
    fn bad_constitutions_say_what_is_wrong() {
        let bad = |rules: Vec<StoredRule>, on_error: &str| {
            let stored = StoredConstitution {
                on_error: on_error.into(),
                max_holds: 3,
                rules,
            };
            format!("{:#}", Constitution::from_stored(stored).unwrap_err())
        };
        assert!(
            bad(vec![rule("A", "t", &[], 0.5, 0.8)], "allow")
                .contains("applies nowhere")
        );
        assert!(
            bad(vec![rule("A", "t", &["bash"], 0.5, 0.8)], "allow")
                .contains("names no target")
        );
        assert!(
            bad(vec![rule("A", "t", &["a.b"], 0.9, 0.5)], "allow")
                .contains("above block")
        );
        let twice = vec![
            rule("A", "t", &["a.b"], 0.5, 0.8),
            rule("A", "u", &["a.b"], 0.5, 0.8),
        ];
        assert!(bad(twice, "allow").contains("Two rules"));
        assert!(bad(Vec::new(), "maybe").contains("not allow or block"));
    }

    #[test]
    fn only_the_fields_a_rule_names_are_checked() {
        let constitution = example();
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
    fn rules_added_are_checked() {
        let mut constitution = example();
        let id = constitution
            .add("Never force-push.", &["bash.command".into()], 0.2, 0.6)
            .unwrap();
        assert_eq!(id, "R1");
        assert!(constitution.add("x", &[], 0.5, 0.8).is_err());
        assert!(constitution.add("x", &["bash".into()], 0.5, 0.8).is_err());
        assert!(constitution.add("x", &["a.b".into()], 0.9, 0.1).is_err());
        assert!(constitution.remove("R2"));
        assert!(!constitution.remove("R2"));
    }
}
