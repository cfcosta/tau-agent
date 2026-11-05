//! The issue graph's laws (ADR 0048, "1. The issue graph"): the plugin's
//! pure Luau, its graph and beads modules, called with drawn graphs and
//! held to the reference model.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod support;

use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use support::{Luau, STATUSES, graph, texts};

thread_local! {
    static LUAU: Luau = Luau::new();
}

fn call(body: &str, input: &Value) -> Value {
    LUAU.with(|luau| luau.call(body, input))
}

/// Ready is the reference model's filter, in its order: priority, then
/// age, then id; and an issue shows blocked exactly when the model says,
/// so closing the last blocker makes it ready in the same call.
#[hegel::test(test_cases = 200)]
fn ready_and_blocked_are_the_models(tc: TestCase) {
    let model = graph(&tc, 8);
    let now: i64 = tc.draw(gs::integers().min_value(0).max_value(10));
    let input = json!({ "issues": model.luau(), "now": now });
    let ready = call("return graph.ready(input.issues, input.now)", &input);
    assert_eq!(texts(&ready), model.ready(now), "{model:?}");
    let blocked = call(
        "local out = {}\n\
         for id in pairs(input.issues) do out[id] = graph.is_blocked(input.issues, id, input.now) end\n\
         return out",
        &input,
    );
    for id in model.issues.keys() {
        assert_eq!(
            blocked[id].as_bool(),
            Some(model.is_blocked(id, now)),
            "{id} in {model:?}"
        );
    }
}

/// A `blocks` link is refused exactly when a way already leads back,
/// and the refusal names a real way: each step a link there is, ending
/// in the link asked for.
#[hegel::test(test_cases = 200)]
fn a_blocks_cycle_is_refused_with_its_path(tc: TestCase) {
    let mut model = graph(&tc, 7);
    let ids: Vec<String> = model.issues.keys().cloned().collect();
    let mut blocker = tc.draw(gs::sampled_from(ids.clone()));
    let mut id = tc.draw(gs::sampled_from(ids.clone()));
    // Sometimes a long way round: a chain of three to six, each blocking
    // the next, and the link asked for would close it.
    if ids.len() >= 3 && tc.draw(gs::booleans()) {
        let length: usize =
            tc.draw(gs::integers().min_value(3).max_value(ids.len().min(6)));
        let chain = &ids[..length];
        for pair in chain.windows(2) {
            let link = (pair[0].clone(), "blocks".to_owned());
            let blocked = model.issues.get_mut(&pair[1]).unwrap();
            if !blocked.links.contains(&link) {
                blocked.links.push(link);
            }
        }
        id = chain[0].clone();
        blocker = chain[length - 1].clone();
    }
    let input = json!({ "issues": model.luau(), "blocker": blocker, "id": id });
    let answer = call(
        "local ok, why = graph.may_block(input.issues, input.blocker, input.id)\n\
         return { ok = ok, why = why }",
        &input,
    );
    let refused = blocker == id || model.reaches(&id, &blocker);
    assert_eq!(answer["ok"].as_bool(), Some(!refused), "{model:?}");
    if refused && blocker != id {
        let why = answer["why"].as_str().unwrap();
        let path = why.strip_prefix("that closes a cycle: ").unwrap();
        let steps: Vec<&str> = path.split(" blocks ").collect();
        assert_eq!(steps.first(), Some(&id.as_str()), "{why}");
        assert_eq!(steps.last(), Some(&id.as_str()), "{why}");
        assert_eq!(steps[steps.len() - 2], blocker, "{why}");
        for pair in steps[..steps.len() - 1].windows(2) {
            let blocked = &model.issues[pair[1]];
            assert!(
                pair[1] == id
                    || blocked
                        .links
                        .contains(&(pair[0].to_owned(), "blocks".to_owned())),
                "{} blocks {} is no link: {why}",
                pair[0],
                pair[1]
            );
        }
    }
}

/// An epic's progress counts exactly its children, and it may close
/// once they all have, not before.
#[hegel::test(test_cases = 150)]
fn an_epic_counts_its_children(tc: TestCase) {
    let model = graph(&tc, 8);
    for (id, issue) in &model.issues {
        let input = json!({ "issues": model.luau(), "id": id });
        let answer = call(
            "local done, all = graph.progress(input.issues, input.id)\n\
             return { done = done, all = all, may = graph.may_close(input.issues, input.id) }",
            &input,
        );
        let (done, all) = model.progress(id);
        assert_eq!(answer["done"], json!(done));
        assert_eq!(answer["all"], json!(all));
        let may = issue.kind != "epic" || done == all;
        assert_eq!(answer["may"].as_bool(), Some(may), "{id} in {model:?}");
    }
}

/// Ids are unique across any creates, with hashes forced to collide;
/// an id deleted never comes back; a child's is its epic's and one past
/// the highest number used.
#[hegel::test(test_cases = 150)]
fn ids_never_repeat(tc: TestCase) {
    let digests: Vec<String> = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            "aaaaaaaa".to_owned(),
            "aaaabbbb".to_owned(),
            "aaaabbbc".to_owned(),
            "12345678".to_owned(),
        ]))
        .min_size(1)
        .max_size(12),
    );
    let deletes: Vec<bool> = tc.draw(
        gs::vecs(gs::booleans())
            .min_size(digests.len())
            .max_size(digests.len()),
    );
    let answer = call(
        "local taken, made, live = {}, {}, {}\n\
         for n, digest in ipairs(input.digests) do\n\
           local id = graph.new_id(taken, 'seed' .. n, function(seed) return digest end)\n\
           taken[id] = true\n\
           table.insert(made, id)\n\
           if input.deletes[n] then live[id] = nil else live[id] = true end\n\
         end\n\
         local epic = made[1]\n\
         local children = {}\n\
         for n = 1, 3 do\n\
           local child = graph.child_id(taken, epic)\n\
           taken[child] = true\n\
           table.insert(children, child)\n\
         end\n\
         return { made = made, children = children }",
        &json!({ "digests": digests, "deletes": deletes }),
    );
    let made = texts(&answer["made"]);
    let unique: BTreeSet<&String> = made.iter().collect();
    assert_eq!(unique.len(), made.len(), "{made:?}");
    for id in &made {
        assert!(id.starts_with("td-") && id.len() >= 7, "{id}");
    }
    let epic = &made[0];
    assert_eq!(
        texts(&answer["children"]),
        [1, 2, 3].map(|n| format!("{epic}.{n}"))
    );
}

/// Every status may go to every other but itself, and a closed one only
/// back to open; a move refused names the status the issue had.
#[test]
fn every_move_is_in_the_table() {
    for from in STATUSES {
        for to in STATUSES {
            let answer = call(
                "local ok, why = graph.may_move({ id = 'td-1', status = input.from }, input.to)\n\
                 return { ok = ok, why = why }",
                &json!({ "from": from, "to": to }),
            );
            let allowed = from != to && (from != "closed" || to == "open");
            assert_eq!(answer["ok"].as_bool(), Some(allowed), "{from} to {to}");
            if !allowed {
                let why = answer["why"].as_str().unwrap();
                assert!(why.contains(from), "{from} to {to}: {why}");
            }
        }
    }
}

/// A drawn issue with every field: criteria, comments, labels, dates
/// and a stored blocked status among them.
fn full(tc: &TestCase, model: &support::Model) -> BTreeMap<String, Value> {
    let words = || {
        gs::vecs(gs::sampled_from(vec![
            "fix", "the", "\"sync\"", "é", "\n", "a\\b",
        ]))
        .max_size(4)
    };
    model
        .issues
        .values()
        .map(|issue| {
            let mut out = support::issue_json(issue);
            let title: Vec<&str> = tc.draw(words());
            out["title"] = json!(format!("t{} {}", issue.id, title.join(" ")));
            let description: Vec<&str> = tc.draw(words());
            out["description"] = json!(description.join(" "));
            let criteria: Vec<bool> = tc.draw(gs::vecs(gs::booleans()).max_size(3));
            out["criteria"] = json!(
                criteria
                    .iter()
                    .enumerate()
                    .map(|(n, done)| json!({ "text": format!("criterion {n}"), "done": done }))
                    .collect::<Vec<_>>()
            );
            let comments: usize = tc.draw(gs::integers().max_value(2));
            out["comments"] = json!(
                (0..comments)
                    .map(|n| json!({ "at": 1_793_700_000 + n as i64, "by": "r1", "text": format!("said {n}\nmore") }))
                    .collect::<Vec<_>>()
            );
            out["labels"] = json!(if tc.draw(gs::booleans()) { vec!["phone", "sync"] } else { vec![] });
            if issue.status == "blocked" && tc.draw(gs::booleans()) {
                out["blocked"] = json!("the vendor");
            }
            if tc.draw(gs::booleans()) {
                out["claim"] = json!("r-claim");
            }
            for field in ["created", "updated"] {
                out[field] = json!(1_793_000_000 + out[field].as_i64().unwrap());
            }
            (issue.id.clone(), out)
        })
        .collect()
}

/// Read back, an export is every field it had; written again, the same
/// bytes.
#[hegel::test(test_cases = 150)]
fn an_export_round_trips(tc: TestCase) {
    let model = graph(&tc, 6);
    let issues = full(&tc, &model);
    let input = json!({ "issues": issues });
    let answer = call(
        "local text = beads.export(input.issues)\n\
         local back = beads.import(text)\n\
         return { text = text, back = back, again = beads.export(back) }",
        &input,
    );
    assert_eq!(answer["again"], answer["text"], "the same bytes");
    // Luau leaves an empty table's kind open: compare as the plugin
    // reads them, an empty list and an empty table alike.
    fn plain(value: &Value) -> Value {
        match value {
            Value::Object(map) if map.is_empty() => json!([]),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), plain(value)))
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.iter().map(plain).collect())
            }
            other => other.clone(),
        }
    }
    assert_eq!(plain(&answer["back"]), plain(&input["issues"]));
    let lines: Vec<&str> = answer["text"].as_str().unwrap().lines().collect();
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted, "sorted by id");
}
