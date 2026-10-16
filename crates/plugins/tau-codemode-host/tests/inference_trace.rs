//! Durable inference provenance.
//!
//! Property inventory: `restored_budget_matches_the_reservation_model`
//! checks owner-scoped budget reconstruction against a direct model of
//! reservations, reported usage and completeness. It catches counting
//! inherited attempts, lost retry charges, totals taken from only the final
//! attempt, and an unfinished, uncertain or incomplete attempt that reopens
//! the budget.
//! Generator plan: up to six traces, each owned or inherited, with up to
//! three reservations and an optional terminal whose attempts match them;
//! shrinking removes traces and attempts and simplifies outcomes toward
//! clean finished ones. The fixed examples below keep the legacy label,
//! malformed records and the budget's own refusal.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::tool::RunId;
use tau_ai::message::{Usage, UsageCost};
use tau_codemode::{
    inference_trace::{self, Attempt, AttemptOutcome, Record, UsageProvenance},
    store,
    ui::State,
};
use tau_codemode_host::{
    CancellationToken,
    inference_budget::{Budget, Limits},
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

/// What one generated trace leaves in the records: whose it is, how many
/// attempts it reserved, and its terminal record, if it wrote one, with
/// one attempt report per reservation.
#[derive(Debug, Clone)]
struct Trace {
    inherited: bool,
    reservations: u32,
    terminal: Option<(bool, Vec<Attempt>)>,
}

hegel::pretty_print_as_debug!(Trace);

const OUTCOMES: [AttemptOutcome; 6] = [
    AttemptOutcome::Finished,
    AttemptOutcome::Cancelled,
    AttemptOutcome::BrokeGrammar,
    AttemptOutcome::NoTerminal,
    AttemptOutcome::OpenFailed,
    AttemptOutcome::Interrupted,
];

const PROVENANCES: [UsageProvenance; 4] = [
    UsageProvenance::SdkReported,
    UsageProvenance::SdkZeroOrDefault,
    UsageProvenance::NoProviderResponse,
    UsageProvenance::Unknown,
];

/// A valid trace: attempts are numbered as they were reserved. Half the
/// attempts are clean (a finished attempt whose usage the SDK reported)
/// so traces that complete are common; the rest draw any outcome and
/// provenance, partial usage included.
#[hegel::composite]
fn trace(tc: &TestCase) -> Trace {
    let reservations = tc.draw(gs::integers::<u32>().max_value(3));
    let terminal = tc.draw(gs::booleans()).then(|| {
        let attempts = (1..=reservations)
            .map(|number| {
                let tokens: u64 = tc.draw(gs::integers::<u64>().max_value(9));
                let (outcome, usage_provenance) = if tc.draw(gs::booleans()) {
                    (AttemptOutcome::Finished, UsageProvenance::SdkReported)
                } else {
                    (
                        OUTCOMES[tc.draw(
                            gs::integers::<usize>()
                                .max_value(OUTCOMES.len() - 1),
                        )],
                        PROVENANCES[tc.draw(
                            gs::integers::<usize>()
                                .max_value(PROVENANCES.len() - 1),
                        )],
                    )
                };
                Attempt {
                    number,
                    outcome,
                    reported_usage: usage(tokens),
                    usage_provenance,
                }
            })
            .collect();
        (tc.draw(gs::booleans()), attempts)
    });
    Trace {
        inherited: tc.draw(gs::booleans()),
        reservations,
        terminal,
    }
}

/// A run's restored budget, from any mix of its own traces and inherited
/// ones, matches a direct model of the rule: every attempt the run
/// reserved counts as a call, whether or not it finished; usage is what
/// its terminal records report, partial usage included; and a later call
/// is blocked when any of its reservations has no terminal, a terminal
/// marked incomplete, or an attempt that did not finish with
/// SDK-reported usage. Inherited traces count for nothing.
#[hegel::test(test_cases = 300)]
fn restored_budget_matches_the_reservation_model(tc: TestCase) {
    let traces: Vec<Trace> = tc.draw(gs::vecs(trace()).max_size(6));
    let mut records = Vec::new();
    let (mut calls, mut tokens, mut incomplete) = (0, 0, false);
    for (index, trace) in traces.iter().enumerate() {
        let id = format!("trace-{index}");
        let run = owner(if trace.inherited { "parent" } else { "run" });
        records.push(stored(started(&id, run.clone())));
        for number in 1..=trace.reservations {
            records.push(stored(Record::Attempt {
                trace_id: id.clone(),
                owner: run.clone(),
                number,
            }));
        }
        let mut reported = 0;
        let mut clean = true;
        if let Some((complete, attempts)) = &trace.terminal {
            let mut total = Usage::default();
            for attempt in attempts {
                total += &attempt.reported_usage;
                reported += attempt.reported_usage.input;
                clean &= attempt.outcome == AttemptOutcome::Finished
                    && matches!(
                        attempt.usage_provenance,
                        UsageProvenance::SdkReported
                            | UsageProvenance::SdkZeroOrDefault
                    );
            }
            clean &= *complete;
            records.push(stored(Record::Finished {
                trace_id: id.clone(),
                owner: run,
                complete: *complete,
                selected: None,
                raw_output: None,
                raw_output_truncated: false,
                error: None,
                attempts: attempts.clone(),
                total_usage: total,
            }));
        } else {
            clean = trace.reservations == 0;
        }
        if !trace.inherited {
            calls += trace.reservations as usize;
            tokens += reported;
            incomplete |= !clean;
        }
    }
    let restored =
        inference_trace::restore_budget(&records, &owner("run")).unwrap();
    assert_eq!(restored.calls, calls);
    assert_eq!(restored.usage.input, tokens);
    assert_eq!(restored.usage.cost.total, tokens as f64);
    assert_eq!(restored.incomplete_attempt, incomplete);
}
