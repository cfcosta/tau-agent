//! Durable inference provenance.
//!
//! Property inventory: `roundtrip_and_restore_match_counter_oracle` checks
//! record serialization and owner-scoped budget reconstruction against a
//! direct counter model. It catches counting inherited attempts, lost retry
//! charges, and totals reconstructed from only the final response.
//! Generator plan: short vectors of small token charges and fork ownership
//! flags shrink to individual attempts. CI uses workspace `hegel.toml`;
//! no per-test count override is needed.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ai::message::{Usage, UsageCost};
use tau_codemode::{
    CancellationToken,
    inference_budget::{Budget, Limits},
    inference_trace::{self, Attempt, AttemptOutcome, Record, UsageProvenance},
    store,
    ui::State,
};

fn owner(id: &str) -> RunId {
    RunId(id.into())
}

fn usage(tokens: u64) -> Usage {
    Usage {
        input: tokens,
        total_tokens: tokens,
        cost: UsageCost {
            total: tokens as f64,
            ..UsageCost::default()
        },
        ..Usage::default()
    }
}

fn stored(record: Record) -> Value {
    serde_json::to_value(store::Record::Inference(record)).unwrap()
}

fn started(trace_id: &str, owner: RunId) -> Record {
    Record::Started {
        trace_id: trace_id.into(),
        owner,
        task: "private task".into(),
        context: json!({"secret": 3}),
        schema: Some(json!({"type": "integer"})),
        model: "test-model".into(),
        effort: Some("medium".into()),
    }
}

fn attempt(number: u32, tokens: u64, outcome: AttemptOutcome) -> Attempt {
    Attempt {
        number,
        outcome,
        reported_usage: usage(tokens),
        usage_provenance: if tokens == 0 {
            UsageProvenance::SdkZeroOrDefault
        } else {
            UsageProvenance::SdkReported
        },
    }
}

fn finished(
    trace_id: &str,
    owner: RunId,
    attempts: Vec<Attempt>,
    error: Option<String>,
) -> Record {
    let mut total = Usage::default();
    for attempt in &attempts {
        total += &attempt.reported_usage;
    }
    Record::Finished {
        trace_id: trace_id.into(),
        owner,
        complete: true,
        selected: error.is_none().then(|| json!("selected")),
        raw_output: Some("private raw output".into()),
        raw_output_truncated: false,
        error,
        attempts,
        total_usage: total,
    }
}

#[test]
fn roundtrip_preserves_private_data_and_ui_defaults() {
    let record = started("opaque", owner("run"));
    let json = stored(record.clone());
    assert_eq!(json["kind"], "inference");
    assert_eq!(json["phase"], "started");
    assert_eq!(json["task"], "private task");
    assert_eq!(
        serde_json::from_value::<store::Record>(json).unwrap(),
        store::Record::Inference(record)
    );
    let old_state: State = serde_json::from_value(json!({
        "store": {}, "writes": 2, "modules": {}
    }))
    .unwrap();
    assert!(old_state.inference.is_empty());
    let mut state = State::default();
    state.inference.push(started("pending", owner("run")));
    assert_eq!(
        state.inference_statuses()["pending"],
        "interrupted / incomplete"
    );
    state
        .inference
        .push(finished("pending", owner("run"), vec![], None));
    assert_eq!(state.inference_statuses()["pending"], "finished");
    assert!(store::fold(&[stored(started("other", owner("run")))]).is_empty());
}

#[test]
fn missing_finish_is_incomplete_and_blocks_same_run() {
    let records = vec![
        stored(started("pending", owner("run"))),
        stored(Record::Attempt {
            trace_id: "pending".into(),
            owner: owner("run"),
            number: 1,
        }),
    ];
    let restored =
        inference_trace::restore_budget(&records, &owner("run")).unwrap();
    assert_eq!(restored.calls, 1);
    assert!(restored.incomplete_attempt);
    assert_eq!(restored.usage, Usage::default());
    let fork =
        inference_trace::restore_budget(&records, &owner("fork")).unwrap();
    assert_eq!(fork.calls, 0);
    assert!(!fork.incomplete_attempt);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .unwrap();
    runtime.block_on(async {
        let budget = Budget::restored(Limits::default(), restored).unwrap();
        assert!(
            budget
                .admit(&CancellationToken::new())
                .await
                .unwrap_err()
                .contains("incomplete inference budget")
        );
    });
}

#[test]
fn started_without_reservation_did_not_reach_provider() {
    let restored = inference_trace::restore_budget(
        &[stored(started("open", owner("run")))],
        &owner("run"),
    )
    .unwrap();
    assert_eq!(restored.calls, 0);
    assert!(!restored.incomplete_attempt);
}

#[test]
fn stored_interrupted_attempt_still_blocks_resume() {
    let mut terminal = finished(
        "interrupted",
        owner("run"),
        vec![Attempt {
            number: 1,
            outcome: AttemptOutcome::Interrupted,
            reported_usage: Usage::default(),
            usage_provenance: UsageProvenance::Unknown,
        }],
        Some("cancelled".into()),
    );
    if let Record::Finished { complete, .. } = &mut terminal {
        *complete = false;
    }
    let records = vec![
        stored(started("interrupted", owner("run"))),
        stored(Record::Attempt {
            trace_id: "interrupted".into(),
            owner: owner("run"),
            number: 1,
        }),
        stored(terminal),
    ];
    let restored =
        inference_trace::restore_budget(&records, &owner("run")).unwrap();
    assert_eq!(restored.calls, 1);
    assert!(restored.incomplete_attempt);
}

#[test]
fn retries_restore_each_reserved_call_and_open_failure_blocks_resume() {
    let attempts = vec![
        attempt(1, 0, AttemptOutcome::Finished),
        attempt(2, 4, AttemptOutcome::Finished),
    ];
    let records = vec![
        stored(started("x", owner("run"))),
        stored(Record::Attempt {
            trace_id: "x".into(),
            owner: owner("run"),
            number: 1,
        }),
        stored(Record::Attempt {
            trace_id: "x".into(),
            owner: owner("run"),
            number: 2,
        }),
        stored(finished("x", owner("run"), attempts, None)),
        stored(started("open-failed", owner("run"))),
        stored(Record::Attempt {
            trace_id: "open-failed".into(),
            owner: owner("run"),
            number: 1,
        }),
        stored(finished(
            "open-failed",
            owner("run"),
            vec![Attempt {
                number: 1,
                outcome: AttemptOutcome::OpenFailed,
                reported_usage: Usage::default(),
                usage_provenance: UsageProvenance::Unknown,
            }],
            Some("open failed".into()),
        )),
    ];
    let restored =
        inference_trace::restore_budget(&records, &owner("run")).unwrap();
    assert_eq!(restored.calls, 3);
    assert_eq!(restored.usage, usage(4));
    assert!(restored.incomplete_attempt);
}

#[test]
fn no_terminal_with_reported_partial_usage_still_blocks_resume() {
    let partial = usage(7);
    for complete in [false, true] {
        let mut terminal = finished(
            "partial",
            owner("run"),
            vec![Attempt {
                number: 1,
                outcome: AttemptOutcome::NoTerminal,
                reported_usage: partial.clone(),
                usage_provenance: UsageProvenance::Unknown,
            }],
            Some("no terminal event".into()),
        );
        if let Record::Finished {
            complete: stored, ..
        } = &mut terminal
        {
            *stored = complete;
        }
        let records = vec![
            stored(started("partial", owner("run"))),
            stored(Record::Attempt {
                trace_id: "partial".into(),
                owner: owner("run"),
                number: 1,
            }),
            stored(terminal),
        ];
        let restored =
            inference_trace::restore_budget(&records, &owner("run")).unwrap();
        assert_eq!(restored.calls, 1);
        assert_eq!(restored.usage, partial);
        assert!(restored.incomplete_attempt);
    }
}

#[test]
fn legacy_open_failure_label_is_still_uncertain() {
    let records = vec![
        stored(started("legacy", owner("run"))),
        stored(Record::Attempt {
            trace_id: "legacy".into(),
            owner: owner("run"),
            number: 1,
        }),
        stored(finished(
            "legacy",
            owner("run"),
            vec![Attempt {
                number: 1,
                outcome: AttemptOutcome::OpenFailed,
                reported_usage: Usage::default(),
                usage_provenance: UsageProvenance::NoProviderResponse,
            }],
            Some("open failed".into()),
        )),
    ];
    let restored =
        inference_trace::restore_budget(&records, &owner("run")).unwrap();
    assert_eq!(restored.calls, 1);
    assert!(restored.incomplete_attempt);
}

#[test]
fn malformed_inference_owner_cannot_be_treated_as_foreign() {
    for bad_owner in [Value::Null, json!(7), json!({"run": "run"})] {
        let mut record = stored(started("uncertain", owner("run")));
        record["owner"] = bad_owner;
        assert!(
            inference_trace::restore_budget(&[record], &owner("run"))
                .unwrap_err()
                .contains("invalid owner")
        );
    }
    let mut missing_owner = stored(started("uncertain", owner("run")));
    missing_owner.as_object_mut().unwrap().remove("owner");
    assert!(
        inference_trace::restore_budget(&[missing_owner], &owner("run"))
            .unwrap_err()
            .contains("invalid owner")
    );
    let foreign = stored(started("foreign", owner("fork")));
    assert_eq!(
        inference_trace::restore_budget(&[foreign], &owner("run")).unwrap(),
        Default::default()
    );
}

#[test]
fn bad_records_never_replenish_the_budget() {
    let base = vec![
        stored(started("x", owner("run"))),
        stored(Record::Attempt {
            trace_id: "x".into(),
            owner: owner("run"),
            number: 1,
        }),
    ];
    for terminal in [
        json!({"kind":"inference","owner":"run","phase":"finished"}),
        stored(finished("x", owner("run"), vec![], None)),
        stored(Record::Attempt {
            trace_id: "x".into(),
            owner: owner("run"),
            number: 1,
        }),
    ] {
        let mut records = base.clone();
        records.push(terminal);
        assert!(
            inference_trace::restore_budget(&records, &owner("run")).is_err()
        );
    }
}

#[test]
fn raw_output_is_bounded_without_confusing_trace_completion() {
    let raw = format!(
        "{}éz",
        "a".repeat(inference_trace::MAX_RAW_OUTPUT_BYTES - 1)
    );
    let (bounded, truncated) = inference_trace::bounded_raw_output(&raw);
    assert!(truncated);
    assert_eq!(bounded.len(), inference_trace::MAX_RAW_OUTPUT_BYTES - 1);
    assert!(!bounded.ends_with('é'));
}

#[hegel::test]
fn roundtrip_and_restore_match_counter_oracle(tc: TestCase) {
    let charges: Vec<(u8, bool)> = tc.draw(
        gs::vecs(gs::tuples2(
            gs::integers::<u8>().max_value(8),
            gs::booleans(),
        ))
        .max_size(12),
    );
    let mut records = Vec::new();
    let mut expected_calls = 0;
    let mut expected_tokens = 0;
    for (index, (tokens, inherited)) in charges.iter().enumerate() {
        let run = if *inherited {
            owner("parent")
        } else {
            owner("run")
        };
        let id = format!("trace-{index}");
        let retry_tokens = u64::from(*tokens) + 1;
        records.push(stored(started(&id, run.clone())));
        for number in 1..=2 {
            records.push(stored(Record::Attempt {
                trace_id: id.clone(),
                owner: run.clone(),
                number,
            }));
        }
        records.push(stored(finished(
            &id,
            run,
            vec![
                attempt(1, u64::from(*tokens), AttemptOutcome::Finished),
                attempt(2, retry_tokens, AttemptOutcome::Finished),
            ],
            None,
        )));
        if !*inherited {
            expected_calls += 2;
            expected_tokens += u64::from(*tokens) + retry_tokens;
        }
    }
    let serialized = serde_json::to_string(&records).unwrap();
    let roundtrip: Vec<Value> = serde_json::from_str(&serialized).unwrap();
    let restored =
        inference_trace::restore_budget(&roundtrip, &owner("run")).unwrap();
    assert_eq!(restored.calls, expected_calls);
    assert_eq!(restored.usage.input, expected_tokens);
    assert_eq!(restored.usage.cost.total, expected_tokens as f64);
    assert!(!restored.incomplete_attempt);
}
