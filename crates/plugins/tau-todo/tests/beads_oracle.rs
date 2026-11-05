//! beads_rust as an oracle (ADR 0048, "6. Oracles"): a drawn graph,
//! exported, imported into `br` in a directory of its own, and `br
//! ready --json` and `br blocked --json` held to ours. Skipped, not
//! failed, where `br` is missing; it runs in the nightly set.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

mod support;

use std::{collections::BTreeSet, path::Path, process::Command};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use support::{Luau, graph, texts};

/// Whether `br` runs here.
fn br_here() -> bool {
    Command::new("br")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success())
}

fn br(dir: &Path, args: &[&str]) -> Value {
    let out = Command::new("br")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "br {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
}

fn ids(value: &Value) -> BTreeSet<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|issue| Some(issue["id"].as_str()?.to_owned()))
        .collect()
}

#[hegel::test(test_cases = 30)]
#[ignore = "nightly"]
fn beads_finds_the_same_ready_and_blocked(tc: TestCase) {
    if !br_here() {
        eprintln!("skipped: `br` (beads_rust) is not installed here");
        return;
    }
    let model = graph(&tc, 6);
    // beads reads no stored deferral past its date as ready: keep dates
    // to come or none.
    let now: i64 = tc.draw(gs::integers().min_value(0).max_value(10));
    let luau = Luau::new();
    let input = json!({ "issues": model.luau(), "now": now });
    let text = luau.call("return beads.export(input.issues)", &input);
    let ready: BTreeSet<String> = texts(
        &luau.call("return graph.ready(input.issues, input.now)", &input),
    )
    .into_iter()
    .collect();
    let blocked: BTreeSet<String> = texts(
        &luau.call("return graph.blocked(input.issues, input.now)", &input),
    )
    .into_iter()
    .collect();
    let dir = tempfile::tempdir().unwrap();
    br(dir.path(), &["init", "--prefix", "td"]);
    let file = dir.path().join("issues.jsonl");
    std::fs::write(&file, text.as_str().unwrap()).unwrap();
    br(dir.path(), &["import", "-i", file.to_str().unwrap()]);
    assert_eq!(ids(&br(dir.path(), &["ready", "--json"])), ready, "ready");
    assert_eq!(
        ids(&br(dir.path(), &["blocked", "--json"])),
        blocked,
        "blocked"
    );
}
