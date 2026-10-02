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
    ToolReply,
    modules::{self, Definition, ModuleTest, TestReport},
    options,
    run,
    store::{self, Writes},
    ui::{self, Action, CodemodeUi, InspectorUi, Row, State},
};
use tau_jev::{Jev, fake::FakeJev};
use tau_store::{Entry, NewRun, RunKind, Store, TurnUsage};
use tau_testing::block_on_io;
use tau_ui_plugin::{
    CallData,
    CallResult,
    CardMark,
    Handle,
    HostCx,
    PluginUi as _,
    Request as UiRequest,
    RunInfo,
    Services,
    UiPlugin as _,
    ViewCx,
    points::AtCard,
};

fn module_record(record: modules::Record) -> Value {
    serde_json::to_value(store::Record::Module(record)).unwrap()
}

fn module_definition(name: &str, source: &str) -> Definition {
    Definition::new(
        name.into(),
        source.into(),
        json!({"run": "() -> number"}),
        BTreeMap::new(),
    )
    .unwrap()
}

// Property inventory: generated define/select/test sequences have the same
// module library in the live UI fold, a reloaded UI fold, and the separate
// persisted-record library oracle at every prefix. hegel.toml sets counts.
#[hegel::test]
fn module_records_match_live_and_reloaded_ui(tc: hegel::TestCase) {
    let steps: Vec<(u8, u8)> = tc.draw(
        gs::vecs(hegel::tuples!(gs::integers::<u8>(), gs::integers::<u8>()))
            .max_size(20),
    );
    let registry = tau_ui_plugin::Registry::new().with(CodemodeUi);
    let plugin = registry.get(PLUGIN).unwrap();
    let mut state = tau_ui_plugin::PluginValue::default();
    let mut records = Vec::new();
    let mut known = Vec::<Definition>::new();
    for (kind, n) in steps {
        let record = if kind % 3 == 0 || known.is_empty() {
            let definition = module_definition(
                if n % 2 == 0 { "alpha" } else { "beta" },
                &format!("return {n}"),
            );
            known.push(definition.clone());
            modules::Record::Define { definition }
        } else {
            let definition = &known[n as usize % known.len()];
            if kind % 3 == 1 {
                modules::Record::Select {
                    name: definition.name().into(),
                    version: definition.version().into(),
                }
            } else {
                let report = TestReport {
                    name: definition.name().into(),
                    version: definition.version().into(),
                    passed: n % 2 == 0,
                    output: format!("case {n}"),
                    output_truncated: false,
                    calls: vec![],
                    error: None,
                    error_truncated: false,
                };
                modules::Record::Test {
                    test: ModuleTest::new(
                        definition.name().into(),
                        definition.version().into(),
                        format!("assert({n} == {n})"),
                        vec![],
                        report,
                    )
                    .unwrap(),
                }
            }
        };
        let body = module_record(record);
        records.push(body.clone());
        plugin.apply(
            &mut state,
            &body,
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
        let oracle = modules::fold(&records);
        assert_eq!(state.get::<State>().modules, oracle);
        let mut reloaded = tau_ui_plugin::PluginValue::default();
        for body in &records {
            plugin.apply(
                &mut reloaded,
                body,
                &mut tau_ui_plugin::testing::FakeRun::default(),
            );
        }
        assert_eq!(reloaded.get::<State>().modules, oracle);
    }
}

#[test]
fn selection_validation_rejects_missing_mismatched_and_corrupt_versions() {
    let definition = module_definition("alpha", "return 1");
    let version = definition.version().to_owned();
    let record = module_record(modules::Record::Define { definition });
    assert_eq!(
        ui::validate_selection(
            std::slice::from_ref(&record),
            "alpha",
            &version
        )
        .unwrap(),
        modules::Record::Select {
            name: "alpha".into(),
            version: version.clone()
        }
    );
    assert!(
        ui::validate_selection(&[], "alpha", &version)
            .unwrap_err()
            .contains("missing")
    );
    assert!(
        ui::validate_selection(std::slice::from_ref(&record), "beta", &version)
            .unwrap_err()
            .contains("another")
    );
    let mut corrupt = record;
    corrupt["definition"]["source"] = json!("return 2");
    assert!(
        ui::validate_selection(&[corrupt], "alpha", &version)
            .unwrap_err()
            .contains("corrupt")
    );
}

#[gpui::test]
fn selection_button_sends_only_the_exact_version(cx: &mut TestAppContext) {
    let requests = Rc::new(std::cell::RefCell::new(Vec::new()));
    let received = requests.clone();
    let handle = Handle::new(
        PLUGIN,
        Rc::new(move |_, request, _: &mut App| {
            received.borrow_mut().push(request)
        }),
    );
    let run = RunId("run-7".into());
    let version = "a".repeat(64);
    cx.update(|cx| ui::select_version(&run, "alpha", &version, &handle, cx));
    assert_eq!(
        *requests.borrow(),
        vec![UiRequest::Act(
            serde_json::to_value(Action::Select {
                run,
                name: "alpha".into(),
                version,
            })
            .unwrap()
        )]
    );
}

#[gpui::test]
fn rejected_selection_requests_a_visible_alert(cx: &mut TestAppContext) {
    let requests = Rc::new(std::cell::RefCell::new(Vec::new()));
    let received = requests.clone();
    let handle = Handle::new(
        PLUGIN,
        Rc::new(move |_, request, _: &mut App| {
            received.borrow_mut().push(request)
        }),
    );
    cx.update(|cx| {
        let ui = cx.new(|cx| InspectorUi::new(handle, cx));
        ui.update(cx, |ui, cx| {
            CodemodeUi.reply(
                ui,
                json!({"error": "That module version is missing"}),
                cx,
            )
        });
    });
    assert_eq!(
        *requests.borrow(),
        vec![UiRequest::Alert {
            title: "Module selection failed".into(),
            message: "That module version is missing".into(),
        }]
    );
}

#[test]
fn host_action_revalidates_persisted_records_and_publishes_selection() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let store = runtime.block_on(Store::memory()).unwrap();
    let run = RunId("module-run".into());
    runtime
        .block_on(store.create_run(&NewRun {
            id: &run.0,
            workflow_id: None,
            agent: "test",
            kind: RunKind::Root,
            model: "test",
            turns: 0,
        }))
        .unwrap();
    let first = module_definition("alpha", "return 1");
    let second = module_definition("alpha", "return 2");
    for definition in [&first, &second] {
        runtime
            .block_on(
                store.append_turn(
                    &run.0,
                    &[Entry::Plugin {
                        plugin: PLUGIN.into(),
                        body: module_record(modules::Record::Define {
                            definition: definition.clone(),
                        })
                        .to_string(),
                    }],
                    TurnUsage::default(),
                ),
            )
            .unwrap();
    }
    let cx = HostCx::new(
        store,
        runtime.handle().clone(),
        Services::default(),
        std::path::PathBuf::from("/tmp/tau-codemode-ui-test"),
        vec![],
        Arc::new(|_| {}),
    );
    let action = |name: &str, version: &str| {
        serde_json::to_value(Action::Select {
            run: run.clone(),
            name: name.into(),
            version: version.into(),
        })
        .unwrap()
    };
    assert!(
        CodemodeUi
            .act(&(), action("alpha", first.version()), &cx)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        modules::fold(&cx.records(&run, PLUGIN).unwrap()).selected()["alpha"],
        first.version()
    );
    let count = cx.records(&run, PLUGIN).unwrap().len();
    for bad in [
        action("alpha", &"f".repeat(64)),
        action("beta", first.version()),
    ] {
        let reply = CodemodeUi.act(&(), bad, &cx).unwrap().unwrap();
        assert!(reply["error"].is_string());
        assert_eq!(cx.records(&run, PLUGIN).unwrap().len(), count);
    }
}

#[test]
fn large_fake_fixture_json_is_bounded_before_display() {
    let value = json!({"fake": "x".repeat(1024 * 1024)});
    let preview = ui::compact_json_preview(&value, 2048);
    assert!(preview.len() <= 2051);
    assert!(preview.ends_with('…'));
}

const CALL: &str = "call_1";

/// What tau-ui folds into a card while its script runs, in the order
/// the script made it: a nested call's start or end, from run events,
/// or one of the call's own updates.
#[derive(Debug, Clone)]
enum Folded {
    Start(ToolCall),
    End(String, ToolOutput, bool),
    Update(Value),
}

impl Folded {
    /// Folds it into `data` as tau-ui's run view does.
    fn apply(&self, data: &mut CallData) {
        match self {
            Self::Start(call) => {
                data.nested_start(&call.id, CALL, &call.name, &call.args)
            }
            Self::End(id, output, error) => data.nested_end(id, output, *error),
            Self::Update(details) => data.updates.push(details.clone()),
        }
    }
}

/// A host that keeps what tau-ui would fold for its calls: each call's
/// start, then its end with the output the loop reports; and the
/// script's updates, its Jev rows.
///
/// - `echo` returns its arguments;
/// - `fail` fails with its `msg`;
/// - `jev.noul` answers 0.5, and a question `bad` gets 1.5, which is out
///   of range.
struct CardHost {
    folded: Mutex<Vec<Folded>>,
    jev: Arc<dyn Jev>,
}

impl Default for CardHost {
    fn default() -> Self {
        Self {
            folded: Mutex::default(),
            jev: Arc::new(FakeJev::nouls(|question| {
                if question == "bad" { 1.5 } else { 0.5 }
            })),
        }
    }
}

impl CardHost {
    /// The card's data with everything folded in the order it came.
    fn data(&self) -> CallData {
        let mut data = CallData::default();
        for folded in self.folded.lock().unwrap().iter() {
            folded.apply(&mut data);
        }
        data
    }
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
        let mut typed = tool("typed");
        typed.output_schema = Some(json!({"type": "object"}));
        vec![tool("echo"), tool("fail"), typed]
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        self.folded
            .lock()
            .unwrap()
            .push(Folded::Start(call.clone()));
        let (output, result) = match call.name.as_str() {
            "fail" => {
                let msg = call.args["msg"].as_str().unwrap_or("").to_owned();
                (ToolOutput::text(msg.clone()), Err(msg))
            }
            "typed" => {
                let msg = call.args["msg"].as_str().unwrap_or("").to_owned();
                let value = json!({"echo": msg});
                let output = ToolOutput {
                    structured: Some(value.clone()),
                    ..ToolOutput::text(msg.clone())
                };
                (
                    output,
                    Ok(ToolReply {
                        value,
                        error: Some(msg),
                    }),
                )
            }
            _ => (
                ToolOutput::text(call.args.to_string()),
                Ok(ToolReply::success(call.args.clone())),
            ),
        };
        let failed = match &result {
            Ok(reply) => reply.error.is_some(),
            Err(_) => true,
        };
        self.folded.lock().unwrap().push(Folded::End(
            call.id.clone(),
            output,
            failed,
        ));
        result
    }

    fn jev(&self) -> Option<Arc<dyn Jev>> {
        Some(self.jev.clone())
    }

    fn update(&self, details: Value) {
        self.folded.lock().unwrap().push(Folded::Update(details));
    }
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

/// One call a generated script makes.
#[derive(Debug, Clone, hegel::DefaultGenerator)]
enum Call {
    /// `echo`, with its argument.
    Echo(String),
    /// `fail`, with its message.
    Fail(String),
    /// A readable structured error, still failed in the call row.
    Typed(String),
    /// `jev.noul`; a bad one gets an answer out of range and raises.
    Jev(bool),
}

impl Call {
    fn luau(&self) -> String {
        match self {
            Self::Echo(text) => {
                format!(
                    "function() return tools.echo({{ s = \"{text}\" }}) end"
                )
            }
            Self::Fail(text) => {
                format!(
                    "function() return tools.fail({{ msg = \"{text}\" }}) end"
                )
            }
            Self::Typed(text) => format!(
                "function() return tools.typed({{ msg = \"{text}\" }}) end"
            ),
            Self::Jev(bad) => format!(
                "function() return jev.noul({{ state = 1, question = \"{}\" }}) end",
                if *bad { "bad" } else { "ok" }
            ),
        }
    }
}

/// Whatever calls a script makes, tools and Jev, in turn or at once,
/// failing or not, the rows its card draws while it runs are the rows
/// its result's details list once it ends, which is all a stored run
/// has; and the card reads the script's output and failure back from
/// the result.
///
/// The nested calls' events and the call's own updates reach tau-ui on
/// two channels, so they may interleave any way: each keeps its own
/// order, and the rows are the same for every interleaving.
#[hegel::test(test_cases = 80)]
fn live_rows_are_the_stored_rows(tc: hegel::TestCase) {
    let text = || gs::text().alphabet("abc xyz").max_size(700);
    let calls: Vec<Call> = tc.draw(
        gs::vecs(
            gs::default::<Call>()
                .echo(text())
                .fail(text())
                .typed(text()),
        )
        .max_size(8),
    );
    let at_once: bool = tc.draw(gs::booleans());
    let output: String = tc.draw(gs::text().alphabet("abc").max_size(20));
    let raise: bool = tc.draw(gs::booleans());
    let mut code = String::new();
    if at_once && !calls.is_empty() {
        let fs: Vec<String> = calls.iter().map(Call::luau).collect();
        code.push_str(&format!("parallel_settled({})\n", fs.join(", ")));
    } else {
        for call in &calls {
            code.push_str(&format!("pcall({})\n", call.luau()));
        }
    }
    code.push_str(&format!("text(\"{output}\")\n"));
    if raise {
        code.push_str("error(\"boom\")\n");
    }
    let host = Arc::new(CardHost::default());
    let outcome = block_on_io(script(&host, &code));
    let (text, details) = result_text(&outcome);
    let stored = ui::stored_rows(&details).unwrap();
    assert_eq!(stored.rows.len(), calls.len());

    // The two channels, each in its order, merged as drawn.
    let folded = host.folded.lock().unwrap().clone();
    let (mut updates, mut events): (Vec<Folded>, Vec<Folded>) = folded
        .into_iter()
        .partition(|folded| matches!(folded, Folded::Update(_)));
    let picks: Vec<bool> = tc.draw(
        gs::vecs(gs::booleans())
            .min_size(updates.len() + events.len())
            .max_size(updates.len() + events.len()),
    );
    updates.reverse();
    events.reverse();
    let mut data = CallData {
        args: json!({ "code": code }),
        ..CallData::default()
    };
    for pick in picks {
        let next = match pick {
            true => updates.pop().or_else(|| events.pop()),
            false => events.pop().or_else(|| updates.pop()),
        };
        next.expect("one per pick").apply(&mut data);
    }
    let live = ui::calls(CALL, &data);
    assert!(
        live.rows
            .iter()
            .all(|row| row.status != CallStatus::Running)
    );
    assert_eq!(timeless(&live.rows), timeless(&stored.rows));
    // In the order they came, too.
    let mut in_order = host.data();
    in_order.args = data.args.clone();
    assert_eq!(
        timeless(&ui::calls(CALL, &in_order).rows),
        timeless(&stored.rows)
    );
    // Jev's cost shows while it runs, as it will once it ended.
    let jev_cost: f64 = live.rows.iter().filter_map(|row| row.cost).sum();
    assert_eq!(jev_cost > 0.0, ui::label(&live, None).contains('$'));

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

/// A Jev request shows on the card as soon as it starts, before any
/// tool call the script makes after it, and its row updates in place
/// when it ends.
#[test]
fn a_jev_request_shows_while_it_runs() {
    let host = Arc::new(CardHost::default());
    let code = "pcall(function() return jev.noul({ state = 1, question = 'ok' }) end)\n\
                tools.echo({ s = 'a' })\n\
                jev.noul({ state = 2, question = 'ok' })";
    block_on_io(script(&host, code));
    let folded = host.folded.lock().unwrap().clone();
    // Only the first update: the request has started, nothing else.
    let mut data = CallData::default();
    folded[0].apply(&mut data);
    let rows = ui::calls(CALL, &data).rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].id.as_str(), rows[0].name.as_str(), rows[0].status),
        ("call_1/jev/1", "jev.noul", CallStatus::Running)
    );
    // Everything: the rows in the order the script made the calls.
    let data = host.data();
    let rows = ui::calls(CALL, &data).rows;
    let names: Vec<(&str, CallStatus)> = rows
        .iter()
        .map(|row| (row.id.as_str(), row.status))
        .collect();
    assert_eq!(
        names,
        [
            ("call_1/jev/1", CallStatus::Ok),
            ("call_1/1", CallStatus::Ok),
            ("call_1/jev/2", CallStatus::Ok),
        ]
    );
    assert!(rows[0].cost.is_some_and(|cost| cost > 0.0));
    assert_eq!(data.updates.len(), 4, "a start and an end for each");
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
            serde_json::to_value(store::Record::Store(writes)).unwrap()
        })
        .collect();
    // Folded as the interface folds it: through the registry, which
    // skips what does not read as a record.
    let registry = tau_ui_plugin::Registry::new().with(CodemodeUi);
    let plugin = registry.get(tau_codemode::PLUGIN).unwrap();
    let mut value = tau_ui_plugin::PluginValue::default();
    for record in &records {
        plugin.apply(
            &mut value,
            record,
            &mut tau_ui_plugin::testing::FakeRun::default(),
        );
    }
    let state = value.get::<State>();
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
        let ui = cx.new(|cx| InspectorUi::new(handle.clone(), cx));
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
