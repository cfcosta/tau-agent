//! tau-codemode's UI: a card's calls read the same live and from
//! history, its output and failure read back from the result, the store
//! folds as the plugin folds it, and it keeps to the design language.

use std::{
    collections::BTreeMap,
    rc::Rc,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use gpui::{App, AppContext as _, TestAppContext};
use hegel::generators as gs;
use serde_json::{Value, json};
use tau_agent::tool::{RunId, ToolOutput};
use tau_codemode::{
    CallStatus,
    CancellationToken,
    Host,
    Item,
    Outcome,
    PLUGIN,
    Request,
    ToolCall,
    ToolEntry,
    options,
    run,
    store::{self, Writes},
    ui::{self, CodemodeUi, Row, State},
};
use tau_ui_plugin::{
    CallData,
    CallResult,
    CardMark,
    Handle,
    RunInfo,
    UiPlugin as _,
    ViewCx,
    points::AtCard,
};

const CALL: &str = "call_1";

/// A host whose calls fold into a card's data as tau-ui folds their
/// events: a start, then an end with the output the loop reports.
///
/// - `echo` returns its arguments;
/// - `fail` fails with its `msg`.
#[derive(Default)]
struct CardHost {
    data: Mutex<CallData>,
}

fn tool(name: &str) -> ToolEntry {
    ToolEntry {
        name: name.into(),
        description: String::new(),
        input_schema: json!({ "type": "object", "properties": {} }),
        output_schema: None,
        namespace: None,
        sequential: false,
    }
}

#[async_trait]
impl Host for CardHost {
    fn tools(&self) -> Vec<ToolEntry> {
        vec![tool("echo"), tool("fail")]
    }

    async fn call_tool(&self, call: ToolCall) -> Result<Value, String> {
        self.data
            .lock()
            .unwrap()
            .nested_start(&call.id, CALL, &call.name, &call.args);
        let (output, result) = match call.name.as_str() {
            "fail" => {
                let msg = call.args["msg"].as_str().unwrap_or("").to_owned();
                (ToolOutput::text(msg.clone()), Err(msg))
            }
            _ => (
                ToolOutput::text(call.args.to_string()),
                Ok(call.args.clone()),
            ),
        };
        self.data.lock().unwrap().nested_end(
            &call.id,
            &output,
            result.is_err(),
        );
        result
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

async fn script(host: &Arc<CardHost>, code: &str) -> Outcome {
    run(
        host.clone(),
        Request {
            call_id: CALL.into(),
            source: options::parse(code).expect("the source parses"),
            store: store::Snapshot::new(),
            cancel: CancellationToken::new(),
        },
    )
    .await
}

/// A row as both sources know it: how long it took is known only once
/// the script ended.
fn timeless(rows: &[Row]) -> Vec<Row> {
    rows.iter()
        .map(|row| Row {
            ms: None,
            ..row.clone()
        })
        .collect()
}

/// The result as tau-ui keeps it: its text blocks joined by newlines.
fn result_text(outcome: &Outcome) -> (String, Value) {
    let rendered = outcome.render(10_000);
    let text = rendered
        .content
        .iter()
        .filter_map(|item| match item {
            Item::Text(text) => Some(text.as_str()),
            Item::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (text, rendered.details)
}

/// Whatever calls a script makes, in turn or at once, failing or not,
/// the rows its card draws from the run's events while it runs are the
/// rows its result's details list once it ends, which is all a stored
/// run has; and the card reads the script's output and failure back
/// from the result.
#[hegel::test(test_cases = 60)]
fn live_rows_are_the_stored_rows(tc: hegel::TestCase) {
    // (fails?, text): echo's argument, or fail's message.
    let calls: Vec<(bool, String)> = tc.draw(
        gs::vecs(hegel::tuples!(
            gs::booleans(),
            gs::text().alphabet("abc xyz").max_size(700),
        ))
        .max_size(8),
    );
    let at_once: bool = tc.draw(gs::booleans());
    let output: String = tc.draw(gs::text().alphabet("abc").max_size(20));
    let raise: bool = tc.draw(gs::booleans());
    let call = |(fails, text): &(bool, String)| {
        if *fails {
            format!("function() return tools.fail({{ msg = \"{text}\" }}) end")
        } else {
            format!("function() return tools.echo({{ s = \"{text}\" }}) end")
        }
    };
    let mut code = String::new();
    if at_once && !calls.is_empty() {
        let fs: Vec<String> = calls.iter().map(call).collect();
        code.push_str(&format!("parallel_settled({})\n", fs.join(", ")));
    } else {
        for c in &calls {
            code.push_str(&format!("pcall({})\n", call(c)));
        }
    }
    code.push_str(&format!("text(\"{output}\")\n"));
    if raise {
        code.push_str("error(\"boom\")\n");
    }
    let host = Arc::new(CardHost::default());
    let outcome = block_on(script(&host, &code));
    let mut data = host.data.lock().unwrap().clone();
    data.args = json!({ "code": code });
    let live = ui::calls(CALL, &data);
    assert!(
        live.rows
            .iter()
            .all(|row| row.status != CallStatus::Running)
    );
    let (text, details) = result_text(&outcome);
    let stored = ui::stored_rows(&details).unwrap();
    assert_eq!(timeless(&live.rows), timeless(&stored.rows));
    assert_eq!(live.rows.len(), calls.len());
    // The call ends: the card draws from its result, as from history.
    data.result = Some(CallResult {
        text: text.clone(),
        details: Some(details.clone()),
        error: outcome.is_error(),
    });
    data.end();
    assert_eq!(ui::calls(CALL, &data), stored);
    let said = ui::said(&text, outcome.is_error());
    assert_eq!(said.output, output);
    assert_eq!(
        said.error,
        outcome.failure.as_ref().map(|failure| failure.head())
    );
    assert_eq!(said.error.is_some(), raise);
}

/// A call that fails before its script runs says only why.
#[test]
fn a_call_that_never_ran_says_why() {
    let said = ui::said("@options must be a JSON object.", true);
    assert_eq!(said.output, "");
    assert_eq!(
        said.error.as_deref(),
        Some("@options must be a JSON object.")
    );
    assert_eq!(ui::stored_rows(&Value::Null), None);
}

/// A plugin's verdict on a call shows on its row, live and once the
/// script ended.
#[test]
fn verdicts_mark_their_rows() {
    let mut data = CallData::default();
    data.nested_start("call_1/1", CALL, "bash", &json!({ "command": "rm" }));
    data.mark_nested(
        "call_1/1",
        "tau-constitution",
        CardMark::Blocked {
            reason: "rule R1".into(),
        },
    );
    data.nested_end("call_1/1", &ToolOutput::text("blocked"), true);
    // A call another call made is that call's to show.
    data.nested_start("call_1/1/1", "call_1/1", "read", &json!({}));
    let live = ui::calls(CALL, &data);
    assert_eq!(live.rows.len(), 1);
    assert_eq!(live.rows[0].marks.len(), 1);
    data.result = Some(CallResult {
        text: "Script completed\nWall time 0.0 seconds\nOutput:\n".into(),
        details: Some(json!({
            "calls": [{ "id": "call_1/1", "name": "bash", "args": "{}",
                        "status": "error", "ms": 3, "error": "blocked",
                        "cost": null }],
            "complete": true, "store": null, "usage": {}, "wall_ms": 12,
        })),
        error: false,
    });
    data.end();
    let stored = ui::calls(CALL, &data);
    assert_eq!(stored.rows[0].marks, live.rows[0].marks);
    assert_eq!(
        ui::label(&stored, data.result.as_ref().unwrap().details.as_ref()),
        "1 call · 0.0 s"
    );
}

/// The store a run shows is the store its records fold to, whatever
/// scripts wrote, records of another shape skipped.
#[hegel::test(test_cases = 200)]
fn the_store_folds_as_the_plugin_folds_it(tc: hegel::TestCase) {
    // (junk?, writes): a key set to a value, or deleted.
    type Ops = Vec<(String, Option<i64>)>;
    let writes: Vec<(bool, Ops)> = tc.draw(gs::vecs(hegel::tuples!(
        gs::booleans(),
        gs::vecs(hegel::tuples!(
            gs::sampled_from(vec!["a".to_owned(), "b".into(), "c".into()]),
            gs::optional(gs::integers::<i64>()),
        ))
        .max_size(4),
    )));
    let records: Vec<Value> = writes
        .iter()
        .map(|(junk, ops)| {
            if *junk {
                return json!({ "kind": "other" });
            }
            let mut writes = Writes::default();
            for (key, value) in ops {
                match value {
                    Some(value) => {
                        writes.delete.retain(|k| k != key);
                        writes.set.insert(key.clone(), json!(value));
                    }
                    None => {
                        writes.set.remove(key);
                        writes.delete.push(key.clone());
                    }
                }
            }
            writes.to_record()
        })
        .collect();
    let mut state = State::default();
    for record in &records {
        state.apply(record);
    }
    assert_eq!(state.store, store::fold(&records));
    assert_eq!(
        state.writes,
        writes.iter().filter(|(junk, _)| !junk).count()
    );
}

/// Jev is optional: the catalog says what it adds, with a key or not.
#[test]
fn the_catalog_entry_says_jev_is_optional() {
    assert_eq!(
        ui::description(true),
        "Runs Luau scripts that call tools, with Jev"
    );
    assert!(
        ui::description(false).starts_with("Runs Luau scripts that call tools")
    );
    assert_eq!(CodemodeUi.name(), PLUGIN);
}

fn info() -> RunInfo {
    RunInfo {
        id: RunId("r".into()),
        repo: "tau-agent".into(),
        live: true,
        title: "run".into(),
        answer: None,
        context: 0,
        window: None,
    }
}

fn at_card(tool: &str, data: CallData) -> AtCard {
    AtCard {
        run: info(),
        call_id: CALL.into(),
        tool: tool.into(),
        keys: Vec::new(),
        data: Arc::new(data),
        summary: String::new(),
        cut: None,
    }
}

/// Draws the card for `at` in a test window, and reads what it says.
fn draw(
    cx: &mut TestAppContext,
    at: &AtCard,
) -> Option<(Option<String>, Option<String>, bool, bool)> {
    let params = BTreeMap::new();
    let repos = BTreeMap::new();
    let list = Vec::new;
    let cards = |_: &RunId| Vec::new();
    let handle = Handle::new(PLUGIN, Rc::new(|_, _, _: &mut App| {}));
    cx.update(|cx| {
        cx.set_global(tau_ui_kit::theme::Theme::graphite());
        let ui = cx.new(|_| ());
        let mut view = ViewCx::new(
            &CodemodeUi,
            ui,
            None,
            &(),
            &(),
            &repos,
            Some(&at.run),
            &params,
            false,
            false,
            1400.,
            handle,
            &list,
            &cards,
            cx,
        );
        ui::card(at, &mut view).map(|card| {
            (card.label, card.failed, card.folds, card.body.is_some())
        })
    })
}

/// The card is codemode's only: open with its calls while the script
/// runs, folded to its header once it ended, and red when it failed.
#[gpui::test]
fn the_card_shows_the_script_as_it_goes(cx: &mut TestAppContext) {
    assert!(draw(cx, &at_card("bash", CallData::default())).is_none());
    let mut data = CallData {
        args: json!({ "code": "-- @options: {}\nreturn tools.read({ path = 'a' })" }),
        ..CallData::default()
    };
    data.nested_start("call_1/1", CALL, "read", &json!({ "path": "a" }));
    let (label, failed, folds, body) =
        draw(cx, &at_card("codemode", data.clone())).unwrap();
    assert_eq!(label.as_deref(), Some("1 call"));
    assert_eq!(failed, None);
    assert!(!folds && body);
    data.result = Some(CallResult {
        text: "Script failed\nWall time 1.5 seconds\nOutput:\n\n\
               Script error:\ncodemode:2: boom\n\nNo tool calls were made."
            .into(),
        details: Some(json!({
            "calls": [], "complete": true, "store": null,
            "usage": { "cost": { "total": 0.0004 } }, "wall_ms": 1500,
        })),
        error: true,
    });
    data.end();
    let (label, failed, folds, _) =
        draw(cx, &at_card("codemode", data)).unwrap();
    assert_eq!(label.as_deref(), Some("0 calls · 1.5 s · $0.0004"));
    assert_eq!(failed.as_deref(), Some("codemode:2: boom"));
    assert!(folds);
}

/// The UI takes its look from the kit.
#[test]
fn only_the_kit_holds_design_values() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let found = tau_ui_kit::design::check(&src, &[]);
    assert!(found.is_empty(), "{}", found.join("\n"));
}
