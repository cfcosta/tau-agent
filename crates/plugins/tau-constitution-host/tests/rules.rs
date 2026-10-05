//! Editing rules against a plain list of rules, and which arguments a
//! call's rules are about: adding takes the lowest free id, a failed
//! edit changes nothing, and what a rule is shown comes from under the
//! field it names, however deep.

use hegel::{
    TestCase,
    extras::serde_json::values,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use serde_json::{Map, Value, json};
use tau_constitution_host::Constitution;

/// A rule as the model keeps it: what was given, trimmed where the
/// constitution trims.
#[derive(Debug, Clone, PartialEq)]
struct Model {
    id: String,
    text: String,
    targets: Vec<String>,
    review: f64,
    block: f64,
}

/// Targets as written in the UI: good ones, spaced ones, and ones that
/// name nothing.
fn target() -> impl PrintableGenerator<String> {
    gs::sampled_from(vec![
        "edit.newText",
        "write.content",
        " bash.command ",
        "a.b.c",
        "final answer",
        "Final Answer",
        "bash",
        ".x",
        "x.",
        "",
    ])
    .map(String::from)
}

/// The label a target reads back as, when it names one.
fn label(target: &str) -> Option<String> {
    let target = target.trim();
    if target.eq_ignore_ascii_case("final answer") {
        return Some("final answer".into());
    }
    let (tool, field) = target.split_once('.')?;
    (!tool.is_empty() && !field.is_empty()).then(|| target.to_owned())
}

/// A threshold, sometimes outside 0 to 1.
fn threshold() -> impl PrintableGenerator<f64> {
    hegel::one_of!(
        gs::floats::<f64>().min_value(0.0).max_value(1.0),
        gs::sampled_from(vec![0.0, 1.0, -0.1, 1.5, f64::NAN]),
    )
}

/// What an add or replace is given.
struct Edit {
    text: String,
    targets: Vec<String>,
    review: f64,
    block: f64,
}

impl Edit {
    /// Mostly a valid rule, so edits go through; otherwise anything.
    fn draw(tc: &TestCase) -> Self {
        if tc.draw(gs::weighted_booleans(0.7)) {
            let review =
                tc.draw(gs::floats::<f64>().min_value(0.0).max_value(1.0));
            return Self {
                text: tc.draw(gs::sampled_from(vec![
                    String::from("Never force-push."),
                    String::from("  No unwrap.  "),
                ])),
                targets: tc.draw(
                    gs::vecs(target().filter(|t| label(t).is_some()))
                        .min_size(1)
                        .max_size(3),
                ),
                review,
                block: tc
                    .draw(gs::floats::<f64>().min_value(review).max_value(1.0)),
            };
        }
        Self {
            text: tc.draw(gs::sampled_from(vec![
                String::from("Never force-push."),
                String::from("   "),
                String::new(),
            ])),
            targets: tc.draw(gs::vecs(target()).max_size(3)),
            review: tc.draw(threshold()),
            block: tc.draw(threshold()),
        }
    }

    /// The rule it makes under `id`, if it is a valid one.
    fn rule(&self, id: &str) -> Option<Model> {
        let probability = |p: f64| (0.0..=1.0).contains(&p);
        let targets: Option<Vec<String>> =
            self.targets.iter().map(|t| label(t)).collect();
        let targets = targets.filter(|targets| !targets.is_empty())?;
        (!self.text.trim().is_empty()
            && probability(self.review)
            && probability(self.block)
            && self.review <= self.block)
            .then(|| Model {
                id: id.to_owned(),
                text: self.text.trim().to_owned(),
                targets,
                review: self.review,
                block: self.block,
            })
    }
}

struct Rules {
    constitution: Constitution,
    model: Vec<Model>,
}

impl Rules {
    /// An id to edit: one that exists, mostly.
    fn id(&self, tc: &TestCase) -> String {
        let mut ids: Vec<String> =
            self.model.iter().map(|rule| rule.id.clone()).collect();
        ids.extend(["R1", "R2", "R9"].map(String::from));
        tc.draw(gs::sampled_from(ids))
    }
}

#[hegel::state_machine]
impl Rules {
    /// Adding takes the lowest `R{n}` no rule has, at the end; a rule
    /// that is not valid is refused and changes nothing.
    #[rule]
    fn add(&mut self, tc: TestCase) {
        let edit = Edit::draw(&tc);
        let free = (1..)
            .map(|n| format!("R{n}"))
            .find(|id| self.model.iter().all(|rule| &rule.id != id))
            .unwrap();
        let added = self.constitution.add(
            &edit.text,
            &edit.targets,
            edit.review,
            edit.block,
        );
        match edit.rule(&free) {
            Some(rule) => {
                tc.event("added");
                assert_eq!(added.unwrap(), free);
                self.model.push(rule);
            }
            None => {
                tc.event("add refused");
                assert!(added.is_err());
            }
        }
    }

    /// Replacing rewrites a rule in place, keeping its id; an unknown id
    /// or a rule that is not valid is refused and changes nothing.
    #[rule]
    fn replace(&mut self, tc: TestCase) {
        let id = self.id(&tc);
        let edit = Edit::draw(&tc);
        let replaced = self.constitution.replace(
            &id,
            &edit.text,
            &edit.targets,
            edit.review,
            edit.block,
        );
        let at = self.model.iter().position(|rule| rule.id == id);
        match (at, edit.rule(&id)) {
            (Some(at), Some(rule)) => {
                tc.event("replaced");
                replaced.unwrap();
                self.model[at] = rule;
            }
            _ => {
                tc.event("replace refused");
                assert!(replaced.is_err());
            }
        }
    }

    /// Removing takes out the rule with that id, and says whether there
    /// was one.
    #[rule]
    fn remove(&mut self, tc: TestCase) {
        let id = self.id(&tc);
        let known = self.model.iter().any(|rule| rule.id == id);
        assert_eq!(self.constitution.remove(&id), known);
        self.model.retain(|rule| rule.id != id);
    }

    /// The rules are the model's, in its order, with unique ids and
    /// review at most block, and they come back from the store's shape.
    #[invariant(always_run)]
    fn the_rules_are_the_models(&self, _tc: TestCase) {
        let rules: Vec<Model> = self
            .constitution
            .rules
            .iter()
            .map(|rule| Model {
                id: rule.id.clone(),
                text: rule.text.clone(),
                targets: rule.on.iter().map(|on| on.label()).collect(),
                review: rule.review,
                block: rule.block,
            })
            .collect();
        assert_eq!(rules, self.model);
        for (n, rule) in rules.iter().enumerate() {
            assert!(rule.review <= rule.block);
            assert!(rules[..n].iter().all(|other| other.id != rule.id));
        }
        assert_eq!(
            Constitution::from_stored(self.constitution.to_stored()).unwrap(),
            self.constitution
        );
    }
}

/// Adding, replacing and removing rules does what the same edits do to
/// a plain list of rules.
#[hegel::test]
fn rules_are_edited_like_a_list(tc: TestCase) {
    let rules = Rules {
        constitution: Constitution::default(),
        model: Vec::new(),
    };
    hegel::stateful::machine(rules).steps(30).run(tc);
}

/// Field names a call's arguments use, so rules find some.
const KEYS: [&str; 4] = ["k", "x", "path", "edits"];

/// Arguments: objects and arrays up to `depth` deep, keyed from
/// [`KEYS`], with any JSON at the leaves.
#[hegel::composite]
fn arguments(tc: &TestCase, depth: usize) -> Value {
    let kind = if depth == 0 {
        0
    } else {
        tc.draw(gs::integers::<u8>().max_value(2))
    };
    match kind {
        0 => tc.draw(values()),
        1 => {
            let mut map = Map::new();
            for _ in 0..tc.draw(gs::integers::<usize>().max_value(3)) {
                let key = tc.draw(gs::sampled_from(KEYS.to_vec()));
                map.insert(key.to_owned(), tc.draw(arguments(depth - 1)));
            }
            Value::Object(map)
        }
        _ => Value::Array(tc.draw(gs::vecs(arguments(depth - 1)).max_size(3))),
    }
}

/// Every value under `key` anywhere in `value`, nested ones included.
fn everything_under(value: &Value, key: &str, found: &mut Vec<Value>) {
    match value {
        Value::Object(map) => {
            for (name, inner) in map {
                if name == key {
                    found.push(inner.clone());
                }
                everything_under(inner, key, found);
            }
        }
        Value::Array(items) => {
            for item in items {
                everything_under(item, key, found);
            }
        }
        _ => {}
    }
}

/// One rule on `tool.k`, one on another tool, one on the final answer.
fn on_k() -> Constitution {
    let mut constitution = Constitution::default();
    for on in ["tool.k", "other.k", "final answer"] {
        constitution
            .add("A rule.", &[on.to_owned()], 0.5, 0.8)
            .unwrap();
    }
    constitution
}

/// A call's rule is shown something exactly when its field occurs in
/// the arguments, and what it is shown was under that field: the one
/// value, or each of several. Rules on other tools or the final answer
/// are never about a call.
#[hegel::test]
fn a_rule_is_shown_what_is_under_its_field(tc: TestCase) {
    let args = tc.draw(arguments(3));
    let constitution = on_k();
    let found = constitution.for_call("tool", &args);
    let mut under = Vec::new();
    everything_under(&args, "k", &mut under);
    assert_eq!(found.len(), usize::from(!under.is_empty()));
    let Some((rule, fields)) = found.first() else {
        tc.event("no field");
        return;
    };
    assert_eq!(rule.id, "R1");
    assert_eq!(fields.keys().collect::<Vec<_>>(), ["k"]);
    let shown = &fields["k"];
    if under.contains(shown) {
        tc.event("one value");
    } else {
        tc.event("several values");
        let Value::Array(each) = shown else {
            panic!("{shown} is not under k in {args}");
        };
        assert!(each.len() >= 2);
        assert!(each.iter().all(|value| under.contains(value)));
    }
}

/// Where the field sits does not matter: arguments wrapped under
/// another name, or in an array, show a rule the same values.
#[hegel::test]
fn wrapping_the_arguments_shows_the_same(tc: TestCase) {
    let args = tc.draw(arguments(3));
    let constitution = on_k();
    let shown = |args: &Value| -> Vec<(String, Map<String, Value>)> {
        constitution
            .for_call("tool", args)
            .into_iter()
            .map(|(rule, fields)| (rule.id.clone(), fields))
            .collect()
    };
    let direct = shown(&args);
    assert_eq!(shown(&json!({ "x": args.clone() })), direct);
    assert_eq!(shown(&json!([args.clone()])), direct);
}

/// A field at the top of the arguments is shown as it is, and what is
/// under it is not searched again.
#[hegel::test]
fn a_top_level_field_is_shown_as_it_is(tc: TestCase) {
    let inner = tc.draw(arguments(3));
    let args = json!({ "k": inner.clone(), "path": "src/lib.rs" });
    let constitution = on_k();
    let found = constitution.for_call("tool", &args);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1["k"], inner);
}
