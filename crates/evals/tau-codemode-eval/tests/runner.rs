//! Inventory: the complete report matches a fixed independent JSON golden;
//! report JSON round trips; the text display loses log evidence while the
//! structured grep sees every failure. The oracle is handwritten output, not
//! the scripts or simulator. No generator is needed for this fixed VM corpus.
//! Hegel's workspace hegel.toml governs the generated fixture properties.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0027)"
)]

use std::process::Command;

use tau_codemode_eval::runner::{EvalReport, Mode, evaluate_offline};

#[tokio::test]
async fn actual_vm_output_matches_independent_golden() {
    let report = evaluate_offline().await.unwrap();
    let actual = serde_json::to_value(&report).unwrap();
    let golden: serde_json::Value =
        serde_json::from_str(include_str!("goldens/offline.json")).unwrap();
    assert_eq!(actual, golden);
    assert_eq!(report.cases[2].mode, Mode::TextOnlyReference);
    assert!(report.cases[2].incomplete);
    assert!(!report.cases[2].correct);
    assert!(report.cases[3].correct);
    assert_eq!(report.cases[5].simulated_round_trips, 1);
    assert!(
        report
            .cases
            .iter()
            .all(|case| case.provider_round_trips == 0
                && case.reported_usage.usd.is_none())
    );
}

#[tokio::test]
async fn typed_report_round_trips_without_changing_results() {
    let report = evaluate_offline().await.unwrap();
    let json = serde_json::to_string(&report).unwrap();
    let restored: EvalReport = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, report);
}

#[test]
fn timings_flag_reports_observed_vm_wall_without_changing_case_results() {
    // Inventory: CLI flag reports a measured value for every case while the
    // independent default report remains the correctness and usage oracle.
    // This fixed VM corpus needs no generator or shrinker; generated fixture
    // properties use the workspace hegel.toml profiles on CI.
    let output = Command::new(env!("CARGO_BIN_EXE_tau-codemode-eval"))
        .arg("--timings")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut measured: EvalReport =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(measured.evidence, "offline_observed_host_runtime");
    assert!(measured.cases.iter().all(|case| case.latency_ms.is_some()));
    for case in &measured.cases {
        assert_eq!(case.provider_round_trips, 0);
        assert_eq!(case.reported_usage, Default::default());
    }
    measured.evidence = "offline_deterministic_scripts".into();
    for case in &mut measured.cases {
        case.latency_ms = None;
    }
    let golden: EvalReport =
        serde_json::from_str(include_str!("goldens/offline.json")).unwrap();
    assert_eq!(measured, golden);
}

#[test]
fn timings_flag_does_not_open_live_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_tau-codemode-eval"))
        .args([
            "--timings",
            "--live",
            "--max-provider-attempts",
            "1",
            "--max-usd",
            "0.01",
            "--max-seconds",
            "1",
            "--max-output-tokens",
            "1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("live evaluation is unsupported")
    );
}
