//! What tau-todo's models share (ADR 0048, "Testing"): the reference
//! model, plain Rust over `BTreeMap`s that answers every question the
//! slow, obvious way; what draws issue graphs; and the plugin's Luau, to
//! call its modules with what was drawn.

#![allow(dead_code, reason = "each test file uses its own part")]

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_luau_plugins_host::runtime::{
    Files,
    Modules,
    PLUGIN,
    evaluate,
    long_string,
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

pub const STATUSES: [&str; 5] =
    ["open", "in_progress", "blocked", "deferred", "closed"];
pub const KINDS: [&str; 4] = ["blocks", "parent", "found_while", "related"];
pub const TYPES: [&str; 5] = ["bug", "feature", "task", "epic", "chore"];

/// One issue, as the model keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub id: String,
    pub kind: String,
    pub priority: i64,
    pub status: String,
    pub created: i64,
    pub defer_until: Option<i64>,
    /// `(id, kind)`: kept on the issue that depends.
    pub links: Vec<(String, String)>,
}

/// The reference model of an issue graph.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Model {
    pub issues: BTreeMap<String, Issue>,
}

impl Model {
    /// Whether `blocker` blocks `id`, read off every link of every issue.
    fn blocks(&self, blocker: &str, id: &str) -> bool {
        self.issues.values().any(|issue| {
            issue.id == id
                && issue
                    .links
                    .iter()
                    .any(|(on, kind)| on == blocker && kind == "blocks")
        })
    }

    /// The open issues that block `id`.
    pub fn open_blockers(&self, id: &str) -> Vec<String> {
        self.issues
            .values()
            .filter(|other| {
                other.status != "closed" && self.blocks(&other.id, id)
            })
            .map(|other| other.id.clone())
            .collect()
    }

    fn waits(&self, issue: &Issue, now: i64) -> bool {
        issue.status == "deferred"
            && issue.defer_until.is_none_or(|at| at > now)
    }

    /// Ready at `now`: open, or deferred past its date, nothing open
    /// blocking it; by priority, then age, then id.
    pub fn ready(&self, now: i64) -> Vec<String> {
        let mut ready: Vec<&Issue> = self
            .issues
            .values()
            .filter(|issue| {
                (issue.status == "open" || issue.status == "deferred")
                    && !self.waits(issue, now)
                    && self.open_blockers(&issue.id).is_empty()
            })
            .collect();
        ready.sort_by(|a, b| {
            (a.priority, a.created, &a.id).cmp(&(b.priority, b.created, &b.id))
        });
        ready.into_iter().map(|issue| issue.id.clone()).collect()
    }

    /// Shown blocked at `now`: stored so, or open with an open blocker.
    pub fn is_blocked(&self, id: &str, now: i64) -> bool {
        let issue = &self.issues[id];
        match issue.status.as_str() {
            "blocked" => true,
            "closed" | "in_progress" => false,
            _ if self.waits(issue, now) => false,
            _ => !self.open_blockers(id).is_empty(),
        }
    }

    /// Whether some way leads from `from` to `to` through `blocks`
    /// links, blocker to blocked: every path walked.
    pub fn reaches(&self, from: &str, to: &str) -> bool {
        fn walk(
            model: &Model,
            at: &str,
            to: &str,
            seen: &mut Vec<String>,
        ) -> bool {
            if at == to {
                return true;
            }
            if seen.iter().any(|s| s == at) {
                return false;
            }
            seen.push(at.to_owned());
            let found = model.issues.keys().any(|next| {
                model.blocks(at, next) && walk(model, next, to, seen)
            });
            seen.pop();
            found
        }
        walk(self, from, to, &mut Vec::new())
    }

    /// An epic's children, and how many are closed.
    pub fn progress(&self, epic: &str) -> (usize, usize) {
        let children: Vec<&Issue> = self
            .issues
            .values()
            .filter(|issue| {
                issue
                    .links
                    .iter()
                    .any(|(on, kind)| on == epic && kind == "parent")
            })
            .collect();
        let done = children
            .iter()
            .filter(|issue| issue.status == "closed")
            .count();
        (done, children.len())
    }

    /// The issues as the plugin keeps them, by id.
    pub fn luau(&self) -> Value {
        Value::Object(
            self.issues
                .values()
                .map(|issue| (issue.id.clone(), issue_json(issue)))
                .collect(),
        )
    }
}

/// `issue` as the plugin keeps it, with every field filled.
pub fn issue_json(issue: &Issue) -> Value {
    let mut out = json!({
        "id": issue.id,
        "title": format!("issue {}", issue.id),
        "description": "",
        "criteria": [],
        "type": issue.kind,
        "priority": issue.priority,
        "status": issue.status,
        "labels": [],
        "comments": [],
        "links": issue.links.iter().map(|(id, kind)| json!({ "id": id, "kind": kind })).collect::<Vec<_>>(),
        "created": issue.created,
        "updated": issue.created,
    });
    if let Some(at) = issue.defer_until {
        out["defer_until"] = json!(at);
    }
    if issue.status == "closed" {
        out["closed"] = json!(issue.created + 1);
        out["close_reason"] = json!("done");
    }
    out
}

/// A graph of one to `most` issues: priorities, ages that tie, statuses,
/// dates past and to come, epics, and links of every kind between them,
/// cycles of `blocks` among them.
pub fn graph(tc: &TestCase, most: usize) -> Model {
    let n: usize = tc.draw(gs::integers().min_value(1).max_value(most));
    let mut model = Model::default();
    for at in 0..n {
        let id = format!("td-{at:04x}");
        let kind = tc.draw(gs::sampled_from(TYPES.to_vec()));
        let status = tc.draw(gs::sampled_from(STATUSES.to_vec()));
        let deferred = status == "deferred" && tc.draw(gs::booleans());
        model.issues.insert(
            id.clone(),
            Issue {
                id,
                kind: kind.to_owned(),
                priority: tc.draw(gs::integers().min_value(0).max_value(4)),
                status: status.to_owned(),
                created: tc.draw(gs::integers().min_value(0).max_value(5)),
                defer_until: deferred.then(|| {
                    tc.draw(gs::integers().min_value(0).max_value(10))
                }),
                links: Vec::new(),
            },
        );
    }
    let ids: Vec<String> = model.issues.keys().cloned().collect();
    let links: usize = tc.draw(gs::integers().max_value(n * 2));
    for _ in 0..links {
        let from = tc.draw(gs::sampled_from(ids.clone()));
        let on = tc.draw(gs::sampled_from(ids.clone()));
        let kind = tc.draw(gs::sampled_from(KINDS.to_vec()));
        if from == on {
            continue;
        }
        let issue = model.issues.get_mut(&from).unwrap();
        let link = (on, kind.to_owned());
        if !issue.links.contains(&link) {
            issue.links.push(link);
        }
    }
    model
}

/// The plugin's Luau, to call its modules.
pub struct Luau {
    modules: Arc<Modules>,
}

impl Luau {
    pub fn new() -> Self {
        let paths: Vec<(String, Vec<u8>)> = tau_todo::FILES
            .iter()
            .map(|(path, source)| {
                ((*path).to_owned(), source.as_bytes().to_vec())
            })
            .collect();
        let files = Files::from_paths(&paths, PLUGIN).unwrap();
        Self {
            modules: Arc::new(files.modules().unwrap()),
        }
    }

    /// What `body`, Luau with `input` decoded as its local `input`,
    /// returns; or why it failed.
    pub fn eval(&self, body: &str, input: &Value) -> Result<Value, String> {
        let script = format!(
            "local graph = require(\"graph\")\n\
             local beads = require(\"beads\")\n\
             local input = json.decode({})\n\
             {body}",
            long_string(&input.to_string())
        );
        block_on_io(evaluate(
            tau_todo::NAME,
            self.modules.clone(),
            script,
            Duration::from_secs(10),
            CancellationToken::new(),
        ))
    }

    pub fn call(&self, body: &str, input: &Value) -> Value {
        self.eval(body, input)
            .unwrap_or_else(|error| panic!("{body}: {error}"))
    }
}

/// `value` as a list of texts: an empty Luau table comes as either.
pub fn texts(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().unwrap().to_owned())
            .collect(),
        Value::Object(map) if map.is_empty() => Vec::new(),
        other => panic!("not a list: {other}"),
    }
}
