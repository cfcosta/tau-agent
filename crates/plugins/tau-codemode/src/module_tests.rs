//! Isolated module tests: only fixture calls and exact registered imports.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::{
    Host,
    Request,
    ToolCall,
    ToolEntry,
    ToolReply,
    modules::{Definition, ExpectedCall, MAX_TEST_REPORT_BYTES, TestReport},
    options::{Options, Source},
    run,
};

const OUTPUT_LIMIT: usize = 64 * 1024;
const ERROR_LIMIT: usize = 16 * 1024;
const TIMEOUT_MS: u64 = 2_000;

struct Calls {
    next: usize,
    attempts: Vec<Value>,
    attempts_total: usize,
    failed: bool,
}

struct FixtureHost {
    root_name: String,
    root_version: String,
    definitions: BTreeMap<String, Definition>,
    fixtures: Vec<ExpectedCall>,
    calls: Mutex<Calls>,
}

impl FixtureHost {
    fn new(
        definition: &Definition,
        available: &BTreeMap<String, Definition>,
        fixtures: Vec<ExpectedCall>,
    ) -> Self {
        let mut definitions = BTreeMap::new();
        let mut pending = vec![definition.clone()];
        while let Some(module) = pending.pop() {
            if definitions.contains_key(module.version()) {
                continue;
            }
            for (name, version) in module.dependencies() {
                if let Some(child) =
                    available.get(version).filter(|child| child.name() == name)
                {
                    pending.push(child.clone());
                }
            }
            definitions.insert(module.version().to_owned(), module);
        }
        Self {
            root_name: definition.name().to_owned(),
            root_version: definition.version().to_owned(),
            definitions,
            fixtures,
            calls: Mutex::new(Calls {
                next: 0,
                attempts: Vec::new(),
                attempts_total: 0,
                failed: false,
            }),
        }
    }

    fn completed(&self) -> (Vec<Value>, bool, usize) {
        let calls = self.calls.lock().expect("not poisoned");
        (calls.attempts.clone(), calls.failed, calls.next)
    }
}

fn preview(value: &Value) -> (String, bool) {
    let text = value.to_string();
    if text.len() <= 128 {
        return (text, false);
    }
    let cut = text
        .char_indices()
        .map(|(at, _)| at)
        .take_while(|at| *at <= 128)
        .last()
        .unwrap_or(0);
    (text[..cut].to_owned(), true)
}

fn truncate_utf8(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    true
}

#[async_trait]
impl Host for FixtureHost {
    fn output_byte_limit(&self) -> usize {
        OUTPUT_LIMIT
    }

    fn tools(&self) -> Vec<ToolEntry> {
        self.fixtures
            .iter()
            .map(|call| call.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|name| ToolEntry {
                name,
                description: "Supplied fake tool".into(),
                input_schema: json!({"type":"object"}),
                output_schema: Some(json!({})),
                namespace: None,
                sequential: false,
            })
            .collect()
    }

    async fn module(
        &self,
        name: &str,
        version: Option<&str>,
    ) -> Result<Option<Definition>, String> {
        if version.is_none() && name == self.root_name {
            return Ok(self.definitions.get(&self.root_version).cloned());
        }
        let Some(version) = version else {
            return Ok(None);
        };
        if name == self.root_name && version == self.root_version {
            return Ok(self.definitions.get(version).cloned());
        }
        Ok(self
            .definitions
            .get(version)
            .filter(|module| module.name() == name)
            .cloned())
    }

    async fn call_tool(&self, call: ToolCall) -> Result<ToolReply, String> {
        let mut calls = self.calls.lock().expect("not poisoned");
        calls.attempts_total += 1;
        let expected = self.fixtures.get(calls.next);
        let (status, error) = match expected {
            None => ("unexpected", Some("unexpected fake call".to_owned())),
            Some(expected)
                if expected.name != call.name || expected.args != call.args =>
            {
                let (args, truncated) = preview(&expected.args);
                let suffix = if truncated { "..." } else { "" };
                (
                    "mismatch",
                    Some(format!(
                        "fake call {} expected {} with {args}{suffix}",
                        calls.next + 1,
                        expected.name
                    )),
                )
            }
            Some(expected) => {
                calls.next += 1;
                if expected.error.is_some() {
                    ("error", expected.error.clone())
                } else {
                    ("ok", None)
                }
            }
        };
        if status == "mismatch" || status == "unexpected" {
            calls.failed = true;
        }
        if calls.attempts.len() < 128 {
            let (args, args_truncated) = preview(&call.args);
            calls.attempts.push(json!({"name":call.name,"args":args,"args_truncated":args_truncated,"status":status,"error":error.as_deref().map(|s| s.chars().take(128).collect::<String>())}));
        } else if calls.attempts.len() == 128 {
            calls
                .attempts
                .push(json!({"status":"truncated","omitted":1}));
        } else {
            let omitted = calls.attempts_total - 128;
            calls.attempts[128]["omitted"] = json!(omitted);
        }
        match (status, expected) {
            ("ok", Some(expected)) => Ok(ToolReply::success(
                expected.value.clone().unwrap_or(Value::Null),
            )),
            ("error", Some(expected)) if expected.value.is_some() => {
                Ok(ToolReply {
                    value: expected.value.clone().unwrap(),
                    error,
                    usage: None,
                    usage_complete: None,
                })
            }
            _ => Err(error.unwrap_or_else(|| "fake call failed".into())),
        }
    }
}

/// Wait for a shared VM slot with cancellation, then run one fresh sandbox.
pub(crate) async fn run_test(
    definition: &Definition,
    available: &BTreeMap<String, Definition>,
    code: String,
    fixtures: Vec<ExpectedCall>,
    cancel: CancellationToken,
    slots: Arc<Semaphore>,
) -> Result<TestReport, String> {
    let _permit = tokio::select! {
        permit = slots.acquire_owned() => permit.map_err(|error| error.to_string())?,
        () = cancel.cancelled() => return Err("module test cancelled while waiting for a VM".into()),
    };
    let host =
        Arc::new(FixtureHost::new(definition, available, fixtures.clone()));
    // Eagerly load the exact target and its pinned graph before user assertions.
    // Keep the original assertion code in ModuleTest; this prelude is runtime-only.
    let source = format!(
        "require('{}', '{}')\n{code}",
        definition.name(),
        definition.version()
    );
    let outcome = run(
        host.clone(),
        Request {
            call_id: "module_test".into(),
            source: Source {
                options: Options {
                    timeout_ms: Some(TIMEOUT_MS),
                    max_output_tokens: None,
                },
                code: source,
            },
            store: Default::default(),
            cancel,
        },
    )
    .await;
    let (calls, failed, consumed) = host.completed();
    let mut output = outcome
        .items
        .iter()
        .map(|item| item.text().unwrap_or("[image omitted]"))
        .collect::<Vec<_>>()
        .join("\n");
    let output_truncated = truncate_utf8(&mut output, OUTPUT_LIMIT);
    let mut error = outcome
        .failure
        .as_ref()
        .map(|failure| failure.head())
        .or_else(|| {
            failed.then(|| {
                "fake calls did not match the supplied expectations".into()
            })
        })
        .or_else(|| {
            (consumed != fixtures.len()).then(|| {
                format!(
                    "{}/{} expected fake calls consumed",
                    consumed,
                    fixtures.len()
                )
            })
        });
    let error_truncated = error
        .as_mut()
        .is_some_and(|error| truncate_utf8(error, ERROR_LIMIT));
    let mut report = TestReport {
        name: definition.name().into(),
        version: definition.version().into(),
        passed: error.is_none(),
        output,
        output_truncated,
        calls,
        error,
        error_truncated,
    };
    // JSON escapes can expand retained text by up to six times (for NUL).
    // Fit the encoded report before ModuleTest validates and saves it.
    while serde_json::to_vec(&report)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_TEST_REPORT_BYTES
    {
        if !report.output.is_empty() {
            let limit = report.output.len() / 2;
            truncate_utf8(&mut report.output, limit);
            report.output_truncated = true;
        } else if let Some(error) =
            report.error.as_mut().filter(|error| !error.is_empty())
        {
            let limit = error.len() / 2;
            truncate_utf8(error, limit);
            report.error_truncated = true;
        } else {
            return Err(
                "module test report diagnostics exceed their bound".into()
            );
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Arc};

    use hegel::{TestCase, generators as gs};
    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::run_test;
    use crate::modules::{Definition, ExpectedCall, Library, Record};

    fn arithmetic() -> (Definition, Library) {
        let definition = Definition::new(
            "arithmetic".into(),
            "return function(a, b) return a * 3 + b * 2 end".into(),
            json!({}),
            BTreeMap::new(),
        )
        .unwrap();
        let mut library = Library::default();
        library
            .apply(&Record::Define {
                definition: definition.clone(),
            })
            .unwrap();
        (definition, library)
    }

    #[tokio::test]
    async fn cancellation_before_slot_does_not_start_a_vm() {
        let (definition, library) = arithmetic();
        let slots = Arc::new(tokio::sync::Semaphore::new(0));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = run_test(
            &definition,
            library.versions(),
            "while true do end".into(),
            vec![],
            cancel,
            slots,
        )
        .await;
        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[tokio::test]
    async fn cancellation_while_two_slots_are_occupied_does_not_leak_a_permit()
    {
        let (definition, library) = arithmetic();
        let slots = Arc::new(tokio::sync::Semaphore::new(2));
        let first = slots.clone().acquire_owned().await.unwrap();
        let second = slots.clone().acquire_owned().await.unwrap();
        assert_eq!(slots.available_permits(), 0);
        let cancel = CancellationToken::new();
        let queued = run_test(
            &definition,
            library.versions(),
            "return true".into(),
            vec![],
            cancel.clone(),
            slots.clone(),
        );
        let stop = async {
            tokio::task::yield_now().await;
            assert_eq!(slots.available_permits(), 0);
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(queued, stop);
        assert!(result.unwrap_err().contains("cancelled while waiting"));
        drop(first);
        drop(second);
        assert_eq!(slots.available_permits(), 2);
        let report = run_test(
            &definition,
            library.versions(),
            "return true".into(),
            vec![],
            CancellationToken::new(),
            slots.clone(),
        )
        .await
        .unwrap();
        assert!(report.passed, "{report:?}");
        assert_eq!(slots.available_permits(), 2);
    }

    #[tokio::test]
    async fn cancellation_stops_an_infinite_loop() {
        let (definition, library) = arithmetic();
        let cancel = CancellationToken::new();
        let work = run_test(
            &definition,
            library.versions(),
            "while true do end".into(),
            vec![],
            cancel.clone(),
            Arc::new(tokio::sync::Semaphore::new(2)),
        );
        let stop = async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            cancel.cancel();
        };
        let (report, ()) = tokio::join!(work, stop);
        let report = report.unwrap();
        assert!(!report.passed);
        assert!(report.error.unwrap().contains("cancelled"));
    }

    #[tokio::test]
    async fn caught_output_limit_still_fails_the_test() {
        let (definition, library) = arithmetic();
        let report = run_test(
            &definition,
            library.versions(),
            "pcall(function() text(string.rep('x', 65537)) end)".into(),
            vec![],
            CancellationToken::new(),
            Arc::new(tokio::sync::Semaphore::new(2)),
        )
        .await
        .unwrap();
        assert!(!report.passed);
        assert!(report.error.unwrap().contains("65536 bytes"));
        assert!(report.output.len() <= 65536);
    }

    #[tokio::test]
    async fn fake_errors_are_raised_or_returned_with_error_status() {
        let (definition, library) = arithmetic();
        let slots = Arc::new(tokio::sync::Semaphore::new(2));
        let raised = run_test(&definition, library.versions(),
            "local ok, message = pcall(function() tools.fake({}) end); assert(not ok and string.find(message, 'blocked'))".into(),
            vec![ExpectedCall { name: "fake".into(), args: json!({}), value: None, error: Some("blocked".into()) }],
            CancellationToken::new(), slots.clone()).await.unwrap();
        assert!(raised.passed, "{raised:?}");
        assert_eq!(raised.calls[0]["status"], "error");
        let structured = run_test(
            &definition,
            library.versions(),
            "assert(tools.fake({}).status == 'down')".into(),
            vec![ExpectedCall {
                name: "fake".into(),
                args: json!({}),
                value: Some(json!({"status":"down"})),
                error: Some("down".into()),
            }],
            CancellationToken::new(),
            slots,
        )
        .await
        .unwrap();
        assert!(structured.passed, "{structured:?}");
        assert_eq!(structured.calls[0]["status"], "error");
    }

    #[tokio::test]
    async fn target_prelude_initializes_once_and_bare_import_uses_test_version()
    {
        let definition = Definition::new(
            "m".into(),
            "tools.fake({phase='init'}); return {value=1}".into(),
            json!({}),
            BTreeMap::new(),
        )
        .unwrap();
        let newer = Definition::new(
            "m".into(),
            "return {value=2}".into(),
            json!({}),
            BTreeMap::new(),
        )
        .unwrap();
        let mut library = Library::default();
        library
            .apply(&Record::Define {
                definition: definition.clone(),
            })
            .unwrap();
        library
            .apply(&Record::Define { definition: newer })
            .unwrap();
        let code = format!(
            "assert(require('m').value == 1); assert(require('m', '{}').value == 1)",
            definition.version()
        );
        let report = run_test(
            &definition,
            library.versions(),
            code,
            vec![ExpectedCall {
                name: "fake".into(),
                args: json!({"phase":"init"}),
                value: Some(json!(true)),
                error: None,
            }],
            CancellationToken::new(),
            Arc::new(tokio::sync::Semaphore::new(2)),
        )
        .await
        .unwrap();
        assert!(report.passed, "{report:?}");
        assert_eq!(report.calls.len(), 1);
        assert_eq!(report.calls[0]["status"], "ok");
    }

    fn fake_fixture(slot: usize, value_case: u8) -> ExpectedCall {
        let (value, error) = match (value_case + slot as u8) % 6 {
            0 => (Some(json!(null)), None),
            1 => (Some(json!(slot as i64 + 10)), None),
            2 => (None, Some("blocked".into())),
            3 => (None, Some(String::new())),
            4 => (Some(json!(null)), Some("blocked".into())),
            _ => (Some(json!({"slot":slot})), Some(String::new())),
        };
        ExpectedCall {
            name: "fake".into(),
            args: json!({"slot":slot}),
            value,
            error,
        }
    }

    fn fake_call_assertion(slot: usize, fixture: &ExpectedCall) -> String {
        let call = format!("tools.fake({{slot={slot}}})");
        match (&fixture.value, &fixture.error) {
            (None, Some(message)) if message.is_empty() => {
                format!(
                    "local ok = pcall(function() {call} end); assert(not ok); "
                )
            }
            (None, Some(message)) => format!(
                "local ok, message = pcall(function() {call} end); assert(not ok and string.find(message, '{message}', 1, true)); "
            ),
            (Some(serde_json::Value::Null), _) => {
                format!("assert({call} == json.null); ")
            }
            (Some(serde_json::Value::Number(number)), _) => {
                format!("assert({call} == {number}); ")
            }
            (Some(serde_json::Value::Object(_)), _) => {
                format!("assert({call}.slot == {slot}); ")
            }
            _ => unreachable!("fixture values have six constructed shapes"),
        }
    }

    // Property inventory: ordered init and callback calls must consume exactly
    // their fixtures; the independent transcript model predicts each attempted
    // name, JSON args, status, and error, plus the final pass bit. Handled tool
    // errors can pass; a mismatched, unexpected, or unused fixture cannot.
    // Counts are built within 0..=3 per phase (at most six calls, seven fixtures).
    // Every case exhausts all four outcome modes. Hegel shrinks counts and
    // values toward short, readable transcripts while preserving each mode.
    #[hegel::test]
    fn ordered_fake_calls_match_the_fixture_transcript_model(tc: TestCase) {
        let init_count: usize =
            tc.draw(gs::integers().min_value(0).max_value(3));
        let drawn_callback_count: usize =
            tc.draw(gs::integers().min_value(0).max_value(3));
        let value_case: u8 = tc.draw(gs::integers().min_value(0).max_value(5));
        for scenario in 0_u8..4 {
            let callback_count = match scenario {
                1 if init_count + drawn_callback_count == 0 => 1,
                2 if init_count + drawn_callback_count < 2 => 2 - init_count,
                _ => drawn_callback_count,
            };
            let call_count = init_count + callback_count;
            let mut fixtures = (0..call_count)
                .map(|slot| fake_fixture(slot, value_case))
                .collect::<Vec<_>>();
            let mut statuses = fixtures
                .iter()
                .map(|fixture| {
                    if fixture.error.is_some() {
                        "error"
                    } else {
                        "ok"
                    }
                })
                .collect::<Vec<_>>();
            match scenario {
                1 => {
                    fixtures.last_mut().unwrap().args = json!({"slot":999});
                    *statuses.last_mut().unwrap() = "mismatch";
                }
                2 => {
                    fixtures.pop();
                    *statuses.last_mut().unwrap() = "unexpected";
                }
                3 => fixtures.push(fake_fixture(call_count, value_case)),
                _ => {}
            }

            let mut root_source = String::new();
            for slot in 0..init_count {
                if (scenario == 1 || scenario == 2) && slot == call_count - 1 {
                    root_source
                        .push_str(&format!("tools.fake({{slot={slot}}}); "));
                } else {
                    root_source.push_str(&fake_call_assertion(
                        slot,
                        &fake_fixture(slot, value_case),
                    ));
                }
            }
            root_source.push_str("return function() ");
            for slot in init_count..call_count {
                if (scenario == 1 || scenario == 2) && slot == call_count - 1 {
                    root_source
                        .push_str(&format!("tools.fake({{slot={slot}}}); "));
                } else {
                    root_source.push_str(&fake_call_assertion(
                        slot,
                        &fake_fixture(slot, value_case),
                    ));
                }
            }
            root_source.push_str("return true end");
            let definition = Definition::new(
                "fixture_model".into(),
                root_source,
                json!({}),
                BTreeMap::new(),
            )
            .unwrap();
            let code = format!(
                "assert(require('fixture_model', '{}')())",
                definition.version()
            );
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let report = runtime
                .block_on(run_test(
                    &definition,
                    &BTreeMap::new(),
                    code,
                    fixtures.clone(),
                    CancellationToken::new(),
                    Arc::new(tokio::sync::Semaphore::new(1)),
                ))
                .unwrap();
            assert_eq!(report.name, "fixture_model");
            assert_eq!(report.version, definition.version());
            assert_eq!(report.passed, scenario == 0, "{report:?}");
            assert_eq!(report.error.is_none(), scenario == 0, "{report:?}");
            assert_eq!(report.calls.len(), call_count, "{report:?}");
            for (slot, call) in report.calls.iter().enumerate() {
                assert_eq!(call["name"], "fake", "{report:?}");
                assert_eq!(
                    call["args"],
                    json!({"slot":slot}).to_string(),
                    "{report:?}"
                );
                assert_eq!(call["args_truncated"], false, "{report:?}");
                assert_eq!(call["status"], statuses[slot], "{report:?}");
                let expected_error = match statuses[slot] {
                    "ok" => None,
                    "mismatch" => Some(format!(
                        "fake call {} expected fake with {}",
                        slot + 1,
                        fixtures[slot].args
                    )),
                    "unexpected" => Some("unexpected fake call".into()),
                    "error" => {
                        Some(fake_fixture(slot, value_case).error.unwrap())
                    }
                    _ => unreachable!(),
                };
                assert_eq!(call["error"], json!(expected_error), "{report:?}");
            }
        }
    }

    #[tokio::test]
    async fn changed_arithmetic_inputs_agree_with_repeated_addition() {
        let (definition, library) = arithmetic();
        let code = format!(
            "local f = require('arithmetic', '{}'); assert(f(4, -3) == 6); assert(f(-2, 5) == 4)",
            definition.version()
        );
        let report = run_test(
            &definition,
            library.versions(),
            code,
            vec![],
            CancellationToken::new(),
            Arc::new(tokio::sync::Semaphore::new(1)),
        )
        .await
        .unwrap();
        assert!(report.passed, "{report:?}");
        assert!(report.calls.is_empty());
    }
}
