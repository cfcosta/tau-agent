//! The reference model of tau-todo's store: each change the tools
//! make, the slow, plain way, over issues as the plugin keeps them.

use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_luau_plugins_host::runtime::Hook;

use super::plain;

pub const RUNS: [&str; 2] = ["r1", "r2"];

/// One call the test makes.
#[derive(Debug, Clone)]
pub enum Op {
    Create {
        title: String,
        kind: String,
        priority: i64,
        criteria: usize,
        parent: Option<String>,
    },
    Priority {
        id: String,
        priority: i64,
    },
    Status {
        id: String,
        status: String,
    },
    Link {
        id: String,
        kind: String,
        on: String,
        remove: bool,
    },
    Claim {
        id: String,
    },
    RunEnd,
    Check {
        id: String,
        at: i64,
    },
    Comment {
        id: String,
        text: String,
    },
    Close {
        id: String,
        reason: String,
    },
    Reopen {
        id: String,
    },
    Defer {
        id: String,
        until: Option<i64>,
    },
    Delete {
        id: String,
    },
}

/// A call, by which run and when.
#[derive(Debug, Clone)]
pub struct Step {
    pub run: String,
    pub now: i64,
    pub op: Op,
}

impl Step {
    /// The hook the plugin is called on, and with what.
    pub fn call(&self) -> (Hook, Value) {
        let tool = |name: &str, args: Value| (Hook::Tool(name.into()), args);
        match &self.op {
            Op::Create {
                title,
                kind,
                priority,
                criteria,
                parent,
            } => {
                let mut args = json!({
                    "title": title,
                    "type": kind,
                    "priority": format!("P{priority}"),
                    "criteria": (0..*criteria).map(|n| format!("c{n}")).collect::<Vec<_>>(),
                });
                if let Some(parent) = parent {
                    args["parent"] = json!(parent);
                }
                tool("todo_create", args)
            }
            Op::Priority { id, priority } => tool(
                "todo_update",
                json!({ "id": id, "priority": format!("P{priority}") }),
            ),
            Op::Status { id, status } => {
                tool("todo_update", json!({ "id": id, "status": status }))
            }
            Op::Link {
                id,
                kind,
                on,
                remove,
            } => tool(
                "todo_link",
                json!({ "id": id, "kind": kind, "on": on, "remove": remove }),
            ),
            Op::Claim { id } => tool("todo_claim", json!({ "id": id })),
            Op::RunEnd => (
                Hook::RunEnd,
                json!({ "stop": "Done", "text": "", "turns": 1, "cost": 0 }),
            ),
            Op::Check { id, at } => {
                tool("todo_check", json!({ "id": id, "criterion": at }))
            }
            Op::Comment { id, text } => {
                tool("todo_comment", json!({ "id": id, "text": text }))
            }
            Op::Close { id, reason } => {
                tool("todo_close", json!({ "id": id, "reason": reason }))
            }
            Op::Reopen { id } => {
                tool("todo_close", json!({ "id": id, "reopen": true }))
            }
            Op::Defer { id, until } => {
                let mut args = json!({ "id": id });
                if let Some(at) = until {
                    args["until"] = json!(at);
                }
                tool("todo_defer", args)
            }
            Op::Delete { id } => {
                (Hook::Action("undo".into()), json!({ "forget": id }))
            }
        }
    }
}

/// The reference model of the store: issues as the plugin keeps them,
/// and the ids retired.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Store {
    pub issues: BTreeMap<String, Value>,
    pub retired: BTreeSet<String>,
}

const MOVES: [(&str, &[&str]); 5] = [
    ("open", &["in_progress", "blocked", "deferred", "closed"]),
    ("in_progress", &["open", "blocked", "deferred", "closed"]),
    ("blocked", &["open", "in_progress", "deferred", "closed"]),
    ("deferred", &["open", "in_progress", "blocked", "closed"]),
    ("closed", &["open"]),
];

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

impl Store {
    fn taken(&self, id: &str) -> bool {
        self.issues.contains_key(id) || self.retired.contains(id)
    }

    /// The issues `run` holds, by id.
    pub fn held_by(&self, run: &str) -> Vec<String> {
        self.issues
            .iter()
            .filter(|(_, issue)| issue["claim"] == json!(run))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn links(issue: &Value) -> Vec<(String, String)> {
        issue["links"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|link| {
                (text(&link["id"]).to_owned(), text(&link["kind"]).to_owned())
            })
            .collect()
    }

    /// Whether a way leads from `from` to `to` through `blocks` links.
    fn reaches(&self, from: &str, to: &str, seen: &mut Vec<String>) -> bool {
        if from == to {
            return true;
        }
        if seen.iter().any(|s| s == from) {
            return false;
        }
        seen.push(from.to_owned());
        let next: Vec<String> = self
            .issues
            .iter()
            .filter(|(_, issue)| {
                Self::links(issue)
                    .contains(&(from.to_owned(), "blocks".to_owned()))
            })
            .map(|(id, _)| id.clone())
            .collect();
        next.iter().any(|id| self.reaches(id, to, seen))
    }

    fn get(&self, id: &str) -> Result<Value, ()> {
        self.issues.get(id).cloned().ok_or(())
    }

    fn save(&mut self, mut issue: Value, now: i64) {
        issue["updated"] = json!(now);
        self.issues
            .insert(text(&issue["id"]).to_owned(), plain(&issue));
    }

    /// `issue` to `status`, as `issues.move` does, or refused.
    fn moved(
        issue: &mut Value,
        status: &str,
        blocked: Option<&str>,
    ) -> Result<(), ()> {
        let from = text(&issue["status"]).to_owned();
        let allowed = MOVES
            .iter()
            .find(|(at, _)| *at == from)
            .is_some_and(|(_, to)| to.contains(&status));
        if !allowed {
            return Err(());
        }
        let map = issue.as_object_mut().unwrap();
        map.insert("status".into(), json!(status));
        if status == "blocked" {
            map.insert(
                "blocked".into(),
                json!(blocked.unwrap_or("something outside tau")),
            );
        } else {
            map.remove("blocked");
        }
        if status != "deferred" {
            map.remove("defer_until");
        }
        if status != "in_progress" {
            map.remove("claim");
        }
        if status != "closed" {
            map.remove("closed");
            map.remove("close_reason");
        }
        Ok(())
    }

    fn child_id(&self, parent: &str) -> String {
        let prefix = format!("{parent}.");
        let highest = self
            .issues
            .keys()
            .chain(&self.retired)
            .filter_map(|id| id.strip_prefix(&prefix)?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        format!("{prefix}{}", highest + 1)
    }

    /// What `step` does here, given what the plugin answered: `Ok` when
    /// it may be done, `Err` when it is refused.
    pub fn apply(
        &mut self,
        step: &Step,
        answer: &Result<Value, String>,
    ) -> Result<(), ()> {
        let now = step.now;
        match &step.op {
            Op::Create {
                title,
                kind,
                priority,
                criteria,
                parent,
            } => {
                let mut links = Vec::new();
                let id = match parent {
                    Some(parent) => {
                        let epic = self.get(parent)?;
                        if epic["type"] != json!("epic") {
                            return Err(());
                        }
                        links.push(json!({ "id": parent, "kind": "parent" }));
                        self.child_id(parent)
                    }
                    None => {
                        let id = text(
                            &answer.as_ref().map_err(|_| ())?["issue"]["id"],
                        )
                        .to_owned();
                        if !id.starts_with("td-") || self.taken(&id) {
                            panic!(
                                "a new issue's id {id} is taken, or not one"
                            );
                        }
                        id
                    }
                };
                if let Some(held) = self.held_by(&step.run).first() {
                    links.push(json!({ "id": held, "kind": "found_while" }));
                }
                let issue = json!({
                    "id": id,
                    "title": title,
                    "description": "",
                    "criteria": (0..*criteria).map(|n| json!({ "text": format!("c{n}"), "done": false })).collect::<Vec<_>>(),
                    "type": kind,
                    "priority": priority,
                    "status": "open",
                    "labels": [],
                    "comments": [],
                    "links": links,
                    "created": now,
                });
                self.save(issue, now);
            }
            Op::Priority { id, priority } => {
                let mut issue = self.get(id)?;
                issue["priority"] = json!(priority);
                self.save(issue, now);
            }
            Op::Status { id, status } => {
                let mut issue = self.get(id)?;
                if issue["status"] != json!(status) {
                    Self::moved(&mut issue, status, None)?;
                }
                self.save(issue, now);
            }
            Op::Link {
                id,
                kind,
                on,
                remove,
            } => {
                let mut issue = self.get(id)?;
                let other = self.get(on)?;
                let mut links = Self::links(&issue);
                let link = (on.clone(), kind.clone());
                let at = links.iter().position(|l| *l == link);
                if *remove {
                    links.remove(at.ok_or(())?);
                } else if at.is_none() {
                    match kind.as_str() {
                        "blocks" => {
                            if on == id || self.reaches(id, on, &mut Vec::new())
                            {
                                return Err(());
                            }
                        }
                        "parent" => {
                            if other["type"] != json!("epic")
                                || links.iter().any(|(_, k)| k == "parent")
                            {
                                return Err(());
                            }
                        }
                        _ => {
                            if on == id {
                                return Err(());
                            }
                        }
                    }
                    links.push(link);
                }
                issue["links"] = json!(
                    links
                        .iter()
                        .map(|(id, kind)| json!({ "id": id, "kind": kind }))
                        .collect::<Vec<_>>()
                );
                self.save(issue, now);
            }
            Op::Claim { id } => {
                let mut issue = self.get(id)?;
                if issue["claim"] == json!(step.run) {
                    return Ok(());
                }
                if !issue["claim"].is_null()
                    || issue["status"] == json!("closed")
                {
                    return Err(());
                }
                Self::moved(&mut issue, "in_progress", None)?;
                issue["claim"] = json!(step.run);
                self.save(issue, now);
            }
            Op::RunEnd => {
                for id in self.held_by(&step.run) {
                    let mut issue = self.get(&id)?;
                    let map = issue.as_object_mut().unwrap();
                    map.remove("claim");
                    if map["status"] == json!("in_progress") {
                        map.insert("status".into(), json!("open"));
                    }
                    let note = format!("The run {} ended (Done).", step.run);
                    map["comments"].as_array_mut().unwrap().push(
                        json!({ "at": now, "by": step.run, "text": note }),
                    );
                    self.save(issue, now);
                }
            }
            Op::Check { id, at } => {
                let mut issue = self.get(id)?;
                let criteria = issue["criteria"].as_array_mut().ok_or(())?;
                let criterion = criteria
                    .get_mut((*at as usize).wrapping_sub(1))
                    .ok_or(())?;
                criterion["done"] = json!(true);
                self.save(issue, now);
            }
            Op::Comment { id, text: said } => {
                let mut issue = self.get(id)?;
                issue["comments"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({ "at": now, "by": step.run, "text": said }));
                self.save(issue, now);
            }
            Op::Close { id, reason } => {
                let mut issue = self.get(id)?;
                if issue["type"] == json!("epic") {
                    let open = self.issues.values().any(|child| {
                        Self::links(child)
                            .contains(&(id.clone(), "parent".into()))
                            && child["status"] != json!("closed")
                    });
                    if open {
                        return Err(());
                    }
                }
                Self::moved(&mut issue, "closed", None)?;
                issue["closed"] = json!(now);
                issue["close_reason"] = json!(reason);
                self.save(issue, now);
            }
            Op::Reopen { id } => {
                let mut issue = self.get(id)?;
                Self::moved(&mut issue, "open", None)?;
                self.save(issue, now);
            }
            Op::Defer { id, until } => {
                let mut issue = self.get(id)?;
                Self::moved(&mut issue, "deferred", None)?;
                if let Some(at) = until {
                    issue["defer_until"] = json!(at);
                }
                self.save(issue, now);
            }
            Op::Delete { id } => {
                self.get(id)?;
                let others: Vec<String> = self.issues.keys().cloned().collect();
                for other in others {
                    let issue = self.issues[&other].clone();
                    let links = Self::links(&issue);
                    let left: Vec<_> =
                        links.iter().filter(|(on, _)| on != id).collect();
                    if left.len() != links.len() {
                        let mut issue = issue;
                        issue["links"] = json!(left.iter().map(|(id, kind)| json!({ "id": id, "kind": kind })).collect::<Vec<_>>());
                        self.save(issue, now);
                    }
                }
                self.issues.remove(id);
                self.retired.insert(id.clone());
            }
        }
        Ok(())
    }
}

/// A step to try, drawn from what there is.
pub fn draw_step(tc: &TestCase, model: &Store, now: &mut i64) -> Step {
    // The clock, moved forward and, now and then, back.
    *now += tc.draw(gs::integers::<i64>().min_value(-30).max_value(120));
    let run = tc.draw(gs::sampled_from(RUNS.to_vec())).to_owned();
    let mut ids: Vec<String> = model.issues.keys().cloned().collect();
    // Now and then an id there is no issue for.
    ids.push("td-none".into());
    let id = || tc.draw(gs::sampled_from(ids.clone()));
    let op = match tc.draw(gs::integers::<u8>().max_value(13)) {
        0..=2 => {
            let epics: Vec<String> = model
                .issues
                .iter()
                .filter(|(_, issue)| issue["type"] == json!("epic"))
                .map(|(id, _)| id.clone())
                .collect();
            let parent = (!epics.is_empty() && tc.draw(gs::booleans()))
                .then(|| tc.draw(gs::sampled_from(epics)));
            Op::Create {
                title: format!("issue {}", tc.draw(gs::integers::<u16>())),
                kind: tc
                    .draw(gs::sampled_from(vec!["task", "bug", "epic"]))
                    .to_owned(),
                priority: tc.draw(gs::integers().min_value(0).max_value(4)),
                criteria: tc.draw(gs::integers().max_value(2)),
                parent,
            }
        }
        3 => Op::Priority {
            id: id(),
            priority: tc.draw(gs::integers().min_value(0).max_value(4)),
        },
        4 => Op::Status {
            id: id(),
            status: tc
                .draw(gs::sampled_from(vec![
                    "open",
                    "in_progress",
                    "blocked",
                    "deferred",
                ]))
                .to_owned(),
        },
        5 | 6 => Op::Link {
            id: id(),
            kind: tc
                .draw(gs::sampled_from(vec![
                    "blocks",
                    "parent",
                    "found_while",
                    "related",
                ]))
                .to_owned(),
            on: id(),
            remove: tc.draw(gs::booleans()),
        },
        7 => Op::Claim { id: id() },
        8 => Op::RunEnd,
        9 => Op::Check {
            id: id(),
            at: tc.draw(gs::integers().min_value(0).max_value(2)),
        },
        10 => Op::Comment {
            id: id(),
            text: format!("said {}", tc.draw(gs::integers::<u8>())),
        },
        11 => Op::Close {
            id: id(),
            reason: "done".into(),
        },
        12 => {
            if tc.draw(gs::booleans()) {
                Op::Reopen { id: id() }
            } else {
                Op::Defer {
                    id: id(),
                    until: tc.draw(gs::booleans()).then(|| *now + 100),
                }
            }
        }
        _ => Op::Delete { id: id() },
    };
    Step { run, now: *now, op }
}

/// Every order of `steps`, by their positions.
fn orders(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![Vec::new()];
    }
    let mut out = Vec::new();
    for first in 0..n {
        for rest in orders(n - 1) {
            let mut order = vec![first];
            order.extend(
                rest.into_iter()
                    .map(|at| if at >= first { at + 1 } else { at }),
            );
            out.push(order);
        }
    }
    out
}

/// The model after `steps` in some order that explains every answer and
/// leaves what `store` keeps; panics if none does.
pub fn explain(
    model: &Store,
    steps: &[Step],
    answers: &[Result<Value, String>],
    store: &(BTreeMap<String, Value>, Vec<String>),
) -> Store {
    for order in orders(steps.len()) {
        let mut tried = model.clone();
        let fits = order.iter().all(|&at| {
            let ok = tried.clone().apply(&steps[at], &answers[at]).is_ok();
            let fits = ok == answers[at].is_ok();
            if ok {
                tried.apply(&steps[at], &answers[at]).unwrap();
            }
            fits
        });
        let retired: Vec<String> = tried.retired.iter().cloned().collect();
        if fits && tried.issues == store.0 && retired == store.1 {
            return tried;
        }
    }
    panic!(
        "no order of {steps:#?} explains {answers:#?}\n\
         the store keeps {:#?}\nthe model had {model:#?}",
        store.0
    );
}
