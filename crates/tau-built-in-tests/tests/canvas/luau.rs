//! tau-canvas's Luau loads as a plugin and passes its own tests, as tau
//! loads it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_luau_plugins_host::runtime::{Loaded, load};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

pub(crate) fn loaded() -> Loaded {
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::CANVAS)
            .unwrap();
    block_on_io(load(tau_luau_plugins::names::CANVAS, files)).unwrap()
}

#[test]
fn the_plugin_loads_and_passes_its_tests() {
    let loaded = loaded();
    assert_eq!(loaded.declaration.name, tau_luau_plugins::names::CANVAS);
    assert_eq!(loaded.declaration.uses.files, ["workspace", "~"]);
    let tests = block_on_io(loaded.test(CancellationToken::new()));
    assert!(tests.len() > 8, "{tests:?}");
    let failed: Vec<_> = tests.iter().filter(|test| !test.passed).collect();
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The blob the demo's fixtures name for its chart: the demo keeps the
/// chart and puts the blob it got in its place.
const DEMO_BLOB: &str = "demo-chart";

const PLAN: &str = "## What changes\n\n\
- `canvas_show` keeps a copy of the image in the repository's blobs, so \
it still shows once the file changes.\n\
- `canvas_write` and `canvas_edit` keep every revision of a document.\n\
- The pane opens beside the transcript when something new arrives.\n\n\
## Open questions\n\n\
1. Should the person edit a document in place?\n\
2. Should uploads land on the canvas too?\n";

/// `hook` with `input` as the call `call` of the demo's run, on `state`:
/// its value and the state it leaves.
fn step(
    loaded: &Loaded,
    hook: tau_luau_plugins_host::runtime::Hook,
    input: serde_json::Value,
    call: &str,
    state: serde_json::Value,
) -> (serde_json::Value, serde_json::Value) {
    use tau_luau_plugins_host::runtime::{Context, NoReach};
    let context = Context {
        run: serde_json::json!({
            "id": "retry-after", "kind": "main", "repo": "tau-agent", "call": call,
        }),
        now: serde_json::json!({}),
        settings: serde_json::json!({}),
        state,
        secrets: None,
        store: None,
        job: None,
    };
    let outcome = block_on_io(loaded.call(
        &hook,
        input,
        &context,
        std::sync::Arc::new(NoReach),
        CancellationToken::new(),
    ));
    assert_eq!(outcome.error, None, "{hook:?}");
    (outcome.value, outcome.state)
}

/// The demo's run: it showed a chart, wrote a plan, and edited it. The
/// calls as the demo replays them, and the canvas they leave, picked
/// as `pick` says, with what the plugin draws of it.
fn demo(loaded: &Loaded, pick: Option<(u32, u32)>) -> serde_json::Value {
    use serde_json::json;
    use tau_luau_plugins_host::runtime::Hook;
    let image = json!({
        "blob": { "id": DEMO_BLOB, "digest": DEMO_BLOB, "size_bytes": 0 },
        "mime": "image/png",
        "width": 1200,
        "height": 640,
        "path": "target/plots/retry-latency.png",
    });
    let caption =
        "p50 and p99 latency per attempt, before and after the backoff change";
    // A chart only a real file gives: as canvas_show leaves it.
    let state = json!({ "canvas": {
        "items": [{ "id": 1, "title": "Retry latency", "revisions": [{
            "call": "canvas-show",
            "content": { "type": "image", "image": image, "caption": caption },
        }] }],
        "fresh": 1,
        "last": 1,
    } });
    let mut calls = vec![json!({
        "id": "canvas-show",
        "tool": "canvas_show",
        "args": {
            "path": "target/plots/retry-latency.png",
            "title": "Retry latency",
            "caption": caption,
        },
        "text": "Image 1, \"Retry latency\", is on the canvas (1200×640 PNG).",
    })];
    let mut state = state;
    for (id, tool, args) in [
        (
            "canvas-write",
            "canvas_write",
            json!({ "title": "Canvas plan", "text": PLAN }),
        ),
        (
            "canvas-edit",
            "canvas_edit",
            json!({ "item": 2, "edits": [{
            "old": "2. Should uploads land on the canvas too?",
            "new": "2. Uploads land on the canvas too: the person's images go to the model as input.",
        }] }),
        ),
    ] {
        let (text, after) =
            step(loaded, Hook::Tool(tool.into()), args.clone(), id, state);
        state = after;
        calls.push(
            json!({ "id": id, "tool": tool, "args": args, "text": text }),
        );
    }
    if let Some((item, revision)) = pick {
        let fresh = state["canvas"]["fresh"].clone();
        state = step(
            loaded,
            Hook::Action("pick".into()),
            json!({ "item": item, "revision": revision, "fresh": fresh }),
            "",
            state,
        )
        .1;
    }
    let (view, _) = step(
        loaded,
        Hook::View,
        serde_json::Value::Null,
        "",
        state.clone(),
    );
    json!({ "blob": DEMO_BLOB, "calls": calls, "state": state, "view": view })
}

/// Each canvas the demo shows is what the plugin leaves, and each view
/// what it draws from it. `BLESS_FIXTURES=1` writes them anew.
#[test]
fn the_demo_is_what_the_plugin_leaves_and_draws() {
    let loaded = loaded();
    for (name, pick) in [
        ("canvas", None),
        ("canvas-image", Some((1, 1))),
        ("canvas-document", Some((2, 1))),
    ] {
        let drawn = demo(&loaded, pick);
        if std::env::var_os("BLESS_FIXTURES").is_some() {
            let path = crate::fixture_path("tau-canvas", name);
            let text = serde_json::to_string_pretty(&drawn).unwrap() + "\n";
            std::fs::write(path, text).unwrap();
            continue;
        }
        let fixture: serde_json::Value =
            serde_json::from_str(crate::fixture("tau-canvas", name)).unwrap();
        assert_eq!(fixture, drawn, "{name}: run with BLESS_FIXTURES=1");
    }
}

/// What the demo lists of the plugin is what it declares.
/// `BLESS_FIXTURES=1` writes it anew from the plugin.
#[test]
fn the_demo_lists_what_the_plugin_declares() {
    let declared = serde_json::to_value(&loaded().declaration).unwrap();
    if std::env::var_os("BLESS_FIXTURES").is_some() {
        let path = crate::fixture_path("tau-canvas", "declaration");
        let text = serde_json::to_string_pretty(&declared).unwrap() + "\n";
        std::fs::write(path, text).unwrap();
        return;
    }
    let fixture: serde_json::Value =
        serde_json::from_str(crate::fixture("tau-canvas", "declaration"))
            .unwrap();
    assert_eq!(fixture, declared, "run with BLESS_FIXTURES=1");
}
