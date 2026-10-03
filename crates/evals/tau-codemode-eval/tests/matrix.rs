//! Integration regressions keep stable goldens, changed-input checks, artifact
//! reconstruction, and accounting. Private VM properties live in src/matrix.rs.

use std::{collections::BTreeMap, process::Command};

use serde_json::{Value, json};
use tau_codemode::modules::Definition;
use tau_codemode_eval::{
    fixtures,
    matrix::{
        ExternalManifest,
        ManifestDependency,
        REFERENCE_SOURCE,
        SourceOrigin,
        evaluate_matrix,
        load_manifest,
    },
    runner::ReportedUsage,
};

#[test]
fn production_agent_matrix_matches_golden_and_independent_oracles() {
    let report = evaluate_matrix(None, false).unwrap();
    let golden: Value =
        serde_json::from_str(include_str!("goldens/matrix.json")).unwrap();
    // This golden projects stable correctness, answers, call categories, and
    // nullable provider fields. The full CLI report also retains each exact
    // artifact page count, which varies with the fixture byte length.
    let actual = json!({
        "evidence": &report.evidence,
        "provider_attempts": report.provider_attempts,
        "observed_provider_usage": &report.observed_provider_usage,
        "observed_provider_latency_ms": report.observed_provider_latency_ms,
        "observed_provider_cost_usd": report.observed_provider_cost_usd,
        "policy": report.policy_cases.iter().map(|case| json!({
            "fixture":&case.fixture,"mode":&case.mode,"correct":case.correct,
            "tool_calls":case.tool_calls,"simulated_round_trips":case.simulated_round_trips,
            "provider_round_trips":case.provider_round_trips
        })).collect::<Vec<_>>(),
        "module": {
            "origin": &report.modules[0].origin,
            "version": &report.modules[0].version,
            "development_calls": &report.modules[0].development.nested,
            "maintenance_calls": &report.modules[0].maintenance.nested,
            "development_model_operations": report.modules[0].development.simulated_model_operations,
            "maintenance_model_operations": report.modules[0].maintenance.simulated_model_operations,
            "successful_reuse_runs": report.modules[0].amortization.successful_reuse_runs,
            "attempted_reuse_runs": report.modules[0].amortization.attempted_reuse_runs,
            "observed_provider_usage_per_run": &report.modules[0].amortization.observed_provider_usage_per_run,
            "cases": report.modules[0].cases.iter().map(|case| json!({
                "fixture":&case.case.fixture,"correct":case.case.correct,
                "result":&case.case.result,
                "read_calls":case.calls.nested.get("read").copied().unwrap_or(0),
                "grep_calls":case.calls.nested.get("grep").copied().unwrap_or(0),
                "artifact_pages_at_least_two":case.calls.nested.get("artifact_read").copied().unwrap_or(0) >= 2,
                "log_evidence_matches_file":case.log_evidence_matches_file,
                "reported_usage":&case.case.reported_usage,
                "provider_round_trips":case.case.provider_round_trips
            })).collect::<Vec<_>>()
        }
    });
    assert_eq!(actual, golden);
    assert_eq!(report.policy_cases.len(), 16);
    assert_eq!(report.modules.len(), 1);
    let module = &report.modules[0];
    assert!(module.development_passed && module.maintenance_passed);
    assert_eq!(module.cases.len(), 8);
    assert!(module.cases.iter().all(|case| case.case.correct));
    for (case, fixture) in module.cases.iter().zip(fixtures::matrix_corpus()) {
        assert_eq!(case.case.result, Some(fixture.expected));
        if fixture.name.starts_with("complete-test-log") {
            if fixture.name.ends_with("changed") {
                assert_eq!(
                    fixture.files[0].text.find("雪 marker"),
                    Some(8_191)
                );
            }
            assert_eq!(
                case.reconstructed_bytes,
                Some(fixture.files[0].text.len())
            );
            assert_eq!(case.log_evidence_matches_file, Some(true));
            assert!(
                case.calls.nested.get("artifact_read").copied().unwrap_or(0)
                    > 1
            );
            // The oracle scans staged lines independently of module parsing.
            let expected_failures: Vec<Value> = fixture.files[0].text.lines().filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                (fields.next() == Some("FAIL")).then(|| json!({
                    "test": fields.next().unwrap(), "message": fields.next().unwrap()
                }))
            }).collect();
            assert_eq!(expected_failures.len(), 3);
            assert_eq!(case.case.result, Some(json!(expected_failures)));
        }
    }
    let scope = module.scope_checks.as_ref().unwrap();
    assert!(
        scope.owner_run_readable
            && scope.unrelated_run_denied
            && scope.foreign_store_denied
    );
    assert_eq!(module.development.nested.get("module_define"), Some(&1));
    assert_eq!(module.development.nested.get("module_test"), Some(&1));
    assert_eq!(module.development.nested.get("module_select"), Some(&1));
    assert!(module.development.simulated_sdk_usage.as_ref().is_some_and(
        |usage| usage.uncached_input_tokens.is_some()
            && usage.cached_input_tokens.is_some()
            && usage.cache_write_tokens.is_some()
            && usage.output_tokens.is_some()
            && usage.usd.is_none()
    ));
    assert_eq!(module.maintenance.nested.get("module_test"), Some(&1));
    assert_eq!(module.maintenance.nested.get("module_select"), Some(&1));
    assert_eq!(module.amortization.successful_reuse_runs, 8);
    assert_eq!(module.amortization.attempted_reuse_runs, 8);
    let all_reuse: usize =
        module.cases.iter().map(|case| case.calls.total()).sum();
    let setup_calls = module.development.total() + module.maintenance.total();
    assert_eq!(
        module.amortization.cold_cumulative_calls,
        Some(setup_calls + module.cases[0].calls.total())
    );
    assert_eq!(
        module.amortization.warm_cumulative_calls,
        Some(
            module.development.total() + module.maintenance.total() + all_reuse
        )
    );
    assert_eq!(
        module.amortization.calls_per_successful_run,
        Some((setup_calls + all_reuse) as f64 / 8.0)
    );
    assert!(
        module
            .amortization
            .externally_reported_development_per_run
            .is_none()
    );
    assert!(
        module
            .amortization
            .observed_provider_usage_per_run
            .is_none()
    );
    assert!(
        report
            .policy_cases
            .iter()
            .all(|case| case.provider_round_trips == 0
                && case.reported_usage.usd.is_none())
    );
    assert!(
        report
            .policy_cases
            .iter()
            .any(|case| case.fixture == "compatibility-extraction-changed"
                && case.simulated_round_trips == 1
                && !case.correct)
    );
}

fn manifest(source: &str, usage: Option<ReportedUsage>) -> ExternalManifest {
    let definition = Definition::new(
        "external_matrix".into(),
        source.into(),
        json!({}),
        BTreeMap::new(),
    )
    .unwrap();
    ExternalManifest {
        origin: SourceOrigin::Human,
        name: "external_matrix".into(),
        version: definition.version().into(),
        source: source.into(),
        signatures: json!({}),
        dependencies: BTreeMap::new(),
        dependency_sources: Vec::new(),
        development_usage: usage.clone(),
        maintenance_usage: usage,
    }
}

#[test]
fn stale_or_broken_external_module_cannot_claim_reuse() {
    let stale = r#"return function(task) local c=json.decode(tools.read({path=task.path}).text); return {result={command=c.test_command},incomplete=false} end"#;
    let report = evaluate_matrix(Some(&manifest(stale, None)), false).unwrap();
    let external = &report.modules[1];
    assert!(external.development_passed);
    assert!(!external.maintenance_passed);
    assert!(
        external
            .stage_failure
            .as_ref()
            .is_some_and(|reason| reason.starts_with("maintenance:"))
    );
    assert!(external.cases.is_empty());
    assert_eq!(external.amortization.successful_reuse_runs, 0);
    assert_eq!(external.amortization.warm_cumulative_calls, None);

    let broken = "return function(task) this is invalid luau end";
    let report = evaluate_matrix(Some(&manifest(broken, None)), false).unwrap();
    let external = &report.modules[1];
    assert!(!external.development_passed);
    assert!(
        external
            .stage_failure
            .as_ref()
            .is_some_and(|reason| reason.starts_with("development:"))
    );
    assert!(external.cases.is_empty());
    assert_eq!(external.amortization.successful_reuse_runs, 0);
}

#[test]
fn manifest_hash_and_incomplete_usage_fail_closed() {
    let mut supplied = manifest(
        REFERENCE_SOURCE,
        Some(ReportedUsage {
            uncached_input_tokens: Some(16),
            cached_input_tokens: Some(8),
            cache_write_tokens: Some(4),
            output_tokens: None,
            usd: None,
        }),
    );
    supplied.version.replace_range(0..1, "f");
    if supplied.version
        == Definition::new(
            supplied.name.clone(),
            supplied.source.clone(),
            json!({}),
            BTreeMap::new(),
        )
        .unwrap()
        .version()
    {
        supplied.version.replace_range(0..1, "a");
    }
    assert!(supplied.validate().is_err());
    supplied.version = Definition::new(
        supplied.name.clone(),
        supplied.source.clone(),
        json!({}),
        BTreeMap::new(),
    )
    .unwrap()
    .version()
    .into();
    let report = evaluate_matrix(Some(&supplied), false).unwrap();
    assert_eq!(report.modules[1].origin, "external_human_unverified");
    assert_eq!(report.modules[1].version, supplied.version);
    assert!(report.modules[1].cases.iter().all(|case| case.case.correct));
    assert!(
        report.modules[1]
            .amortization
            .externally_reported_development_per_run
            .is_none()
    );
}

#[test]
fn dependency_sources_require_exact_hashes_and_ordered_pins() {
    let child = Definition::new(
        "child".into(),
        "return {value=1}".into(),
        json!({}),
        BTreeMap::new(),
    )
    .unwrap();
    let pins = BTreeMap::from([("child".into(), child.version().into())]);
    let root = Definition::new(
        "external_matrix".into(),
        REFERENCE_SOURCE.into(),
        json!({}),
        pins.clone(),
    )
    .unwrap();
    let mut supplied = manifest(REFERENCE_SOURCE, None);
    supplied.version = root.version().into();
    supplied.dependencies = pins;
    supplied.dependency_sources.push(ManifestDependency {
        name: "child".into(),
        version: child.version().into(),
        source: "return {value=1}".into(),
        signatures: json!({}),
        dependencies: BTreeMap::new(),
    });
    assert!(supplied.validate().is_ok());
    supplied.dependency_sources[0].source = "return {value=2}".into();
    assert!(supplied.validate().is_err());
    supplied.dependency_sources.clear();
    assert!(supplied.validate().is_err());
}

#[test]
fn cli_requires_matrix_for_external_source_without_opening_a_provider() {
    let output = Command::new(env!("CARGO_BIN_EXE_tau-codemode-eval"))
        .args(["--module-manifest", "/does-not-exist"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("--module-manifest requires --matrix")
    );
}

#[test]
fn manifest_file_has_a_bounded_read() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), vec![b' '; 2 * 1024 * 1024 + 1]).unwrap();
    assert!(
        load_manifest(file.path())
            .unwrap_err()
            .contains("exceeds 2 MiB")
    );
}

#[test]
fn manifest_validation_enforces_scratch_quota_and_dependency_order() {
    let mut supplied = manifest(REFERENCE_SOURCE, None);
    supplied.dependency_sources = (0..128)
        .map(|index| ManifestDependency {
            name: format!("child_{index}"),
            version: "0".repeat(64),
            source: "return {}".into(),
            signatures: json!({}),
            dependencies: BTreeMap::new(),
        })
        .collect();
    assert!(
        supplied
            .validate()
            .unwrap_err()
            .contains("127 dependency sources")
    );
    supplied.dependency_sources.clear();
    let source = "x".repeat(60_000);
    for index in 0..18 {
        let name = format!("child_{index}");
        let definition = Definition::new(
            name.clone(),
            source.clone(),
            json!({}),
            BTreeMap::new(),
        )
        .unwrap();
        supplied.dependency_sources.push(ManifestDependency {
            name,
            version: definition.version().into(),
            source: source.clone(),
            signatures: json!({}),
            dependencies: BTreeMap::new(),
        });
    }
    assert!(
        supplied
            .validate()
            .unwrap_err()
            .contains("definitions exceed 1 MiB")
    );
    supplied.dependency_sources.truncate(1);
    let duplicate = supplied.dependency_sources[0].clone();
    supplied.dependency_sources.push(duplicate);
    assert!(
        supplied
            .validate()
            .unwrap_err()
            .contains("duplicate dependency")
    );
    supplied.dependency_sources.truncate(1);
    let version = supplied.dependency_sources[0].version.clone();
    supplied.dependency_sources[0]
        .dependencies
        .insert("child_0".into(), version);
    assert!(supplied.validate().unwrap_err().contains("unavailable"));
    supplied.dependency_sources.clear();
    supplied.development_usage = Some(ReportedUsage {
        usd: Some(f64::INFINITY),
        ..ReportedUsage::default()
    });
    assert!(
        supplied
            .validate()
            .unwrap_err()
            .contains("finite and nonnegative")
    );
}
