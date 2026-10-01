//! The runner with scripted models: each arm carries the first run's
//! memory into the second run's first request the way it should (and
//! `none` carries nothing), a changed fact reaches memory as a stale mark
//! and its use is caught, checks decide success, and the budget stops
//! the evaluation.

use std::{collections::HashMap, sync::Arc};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_ai::{
    llm::Llm,
    message::{InputBlock, Message, UserContent},
};
use tau_memory_e2e::{
    arm::{Arm, MEMORY_MD},
    metrics::Trial,
    runner::{Budget, Config, Report, RunKey, Stage, evaluate},
    scenario::{self, Variant},
};
use tau_testing::{
    block_on_io,
    scripted::{Request, ScriptedModel},
};

/// The scripted model of each run, by arm, variant, trial and stage;
/// a run without one gets an empty script, which fails at once.
#[derive(Default)]
struct Scripts(HashMap<(Arm, Variant, u32, Stage), ScriptedModel>);

impl Scripts {
    fn set(
        &mut self,
        arm: Arm,
        variant: Variant,
        trial: u32,
        stage: Stage,
        model: ScriptedModel,
    ) {
        self.0.insert((arm, variant, trial, stage), model);
    }

    fn get(&self, arm: Arm, variant: Variant, stage: Stage) -> &ScriptedModel {
        &self.0[&(arm, variant, 0, stage)]
    }

    fn model(&self, key: &RunKey) -> Arc<dyn Llm> {
        Arc::new(
            self.0
                .get(&(key.arm, key.variant, key.trial, key.stage))
                .cloned()
                .unwrap_or_default(),
        )
    }
}

fn config(
    scenario: &str,
    variants: &[Variant],
    arms: &[Arm],
    work: &std::path::Path,
) -> Config {
    let mut config = Config::new("scripted", work);
    config.scenarios = vec![scenario::find(scenario).unwrap()];
    config.variants = variants.to_vec();
    config.arms = arms.to_vec();
    config
}

fn run(config: &Config, scripts: &Scripts) -> Report {
    block_on_io(evaluate(config, &|key| scripts.model(key), &mut |_| {}))
        .unwrap()
}

/// The text of a request's first user message: the plugins' context,
/// then the task.
fn first_user_text(request: &Request) -> String {
    match &request.transcript[0] {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|block| match block {
                    InputBlock::Text(text) => Some(text.text.clone()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
        other => panic!("{other:?}"),
    }
}

fn tool_names(request: &Request) -> Vec<String> {
    request
        .settings
        .tools
        .iter()
        .map(|tool| tool.name.clone())
        .collect()
}

/// A word the first runs below learn, and nothing in the repository
/// holds.
const LEARNED: &str = "zanzibar-quokka";

/// What the first run does in each arm to keep what it learned: nothing
/// in `none`, a command in `transcripts`, `MEMORY.md` in `memory_md`, a
/// note in `memory`, and a note from the consolidation pass in
/// `memory_consolidate`.
fn first_run(arm: Arm) -> ScriptedModel {
    let note = json!({
        "type": "gotcha",
        "title": "The test suite needs HARBOR_MODE=test",
        "description": format!("./test.sh refuses to run without it ({LEARNED})"),
        "body": format!("Run HARBOR_MODE=test ./test.sh; without it the suite exits 3. {LEARNED}"),
        "links": [{"to": "ci/env.sh", "type": "about"}],
    });
    let model = ScriptedModel::new();
    match arm {
        Arm::None => model.turn(|t| t.text("done")),
        Arm::Transcripts => model
            .turn(|t| {
                t.tool_call(
                    "bash",
                    json!({"command": format!("echo {LEARNED}; HARBOR_MODE=test ./test.sh")}),
                )
            })
            .turn(|t| t.text("done")),
        Arm::MemoryMd => model
            .turn(|t| {
                t.tool_call(
                    "write",
                    json!({
                        "path": "MEMORY.md",
                        "content": format!("- tests: HARBOR_MODE=test ./test.sh ({LEARNED})\n"),
                    }),
                )
            })
            .turn(|t| t.text("done")),
        Arm::Memory => model
            .turn(move |t| t.tool_call("memory_write", note))
            .turn(|t| t.text("done")),
        // The run itself saves nothing; the pass after it does.
        Arm::MemoryConsolidate => model
            .turn(|t| t.text("done"))
            .turn(move |t| t.tool_call("memory_write", note)),
    }
}

#[test]
fn the_second_run_starts_with_what_its_arm_carries_over() {
    let work = tempfile::tempdir().unwrap();
    let mut scripts = Scripts::default();
    for arm in Arm::ALL {
        scripts.set(arm, Variant::Stable, 0, Stage::First, first_run(arm));
        scripts.set(
            arm,
            Variant::Stable,
            0,
            Stage::Second,
            ScriptedModel::new().turn(|t| t.text("done")),
        );
    }
    let config =
        config("test-mode", &[Variant::Stable], &Arm::ALL, work.path());
    let report = run(&config, &scripts);
    assert_eq!(report.trials.len(), Arm::ALL.len());
    for trial in &report.trials {
        let arm = trial.arm;
        let second =
            &scripts.get(arm, Variant::Stable, Stage::Second).requests()[0];
        let text = first_user_text(second);
        let carried = arm != Arm::None;
        assert_eq!(text.contains(LEARNED), carried, "{}: {text}", arm.name());
        assert_eq!(trial.memory_saved, carried, "{}", arm.name());
        assert_eq!(trial.memory_given, carried, "{}", arm.name());
        assert!(trial.read_memory() == carried);
        // Fenced as data, in the arm's own way.
        let fence = match arm {
            Arm::None => None,
            Arm::MemoryMd => Some("<memory-file"),
            Arm::Transcripts => Some("<transcript"),
            Arm::Memory | Arm::MemoryConsolidate => Some("<memory"),
        };
        if let Some(fence) = fence {
            assert!(text.contains(fence), "{}: {text}", arm.name());
        }
        // Memory tools only in the memory arms; MEMORY.md is asked for
        // only in its arm.
        let tools = tool_names(second);
        assert_eq!(
            tools.contains(&"memory_search".to_owned()),
            arm.uses_memory(),
            "{}: {tools:?}",
            arm.name()
        );
        let instructions = second.settings.instructions.clone().unwrap();
        assert_eq!(
            instructions.contains(MEMORY_MD),
            arm == Arm::MemoryMd,
            "{}",
            arm.name()
        );
    }
    // Consolidation asked once more, after each run, and only in its arm.
    let asked =
        |arm, stage| scripts.get(arm, Variant::Stable, stage).requests().len();
    assert_eq!(asked(Arm::MemoryConsolidate, Stage::First), 2);
    assert_eq!(asked(Arm::MemoryConsolidate, Stage::Second), 2);
    assert_eq!(asked(Arm::Memory, Stage::Second), 1);
}

/// In the changed variant, notes about the changed files come marked
/// may-be-stale, and a second run that still uses the old fact fails its
/// check and is counted as having used it.
#[test]
fn a_changed_fact_is_marked_stale_and_its_use_is_caught() {
    let work = tempfile::tempdir().unwrap();
    let mut scripts = Scripts::default();
    scripts.set(
        Arm::Memory,
        Variant::Changed,
        0,
        Stage::First,
        first_run(Arm::Memory),
    );
    let test_mode = scenario::find("test-mode").unwrap();
    // The stale way: the old variable, and the task otherwise done.
    scripts.set(
        Arm::Memory,
        Variant::Changed,
        0,
        Stage::Second,
        ScriptedModel::new()
            .turn(|t| {
                t.tool_call(
                    "bash",
                    json!({"command": test_mode.second.solution}),
                )
            })
            .turn(|t| t.text("done")),
    );
    let config = config(
        "test-mode",
        &[Variant::Changed],
        &[Arm::Memory],
        work.path(),
    );
    let report = run(&config, &scripts);
    let trial = &report.trials[0];
    let second = &scripts
        .get(Arm::Memory, Variant::Changed, Stage::Second)
        .requests()[0];
    let text = first_user_text(second);
    assert!(text.contains("may be stale"), "{text}");
    assert_eq!(trial.stale_used, Some(true));
    assert_eq!(trial.second.success, Some(false));
    assert_eq!(trial.second.tool_calls, 1);
}

/// Success is the check's verdict on what the run did: a second run that
/// does the task passes, in both variants, with no stale use in the
/// changed one; one that does nothing fails.
#[test]
fn success_is_the_checks_verdict_on_the_repository() {
    let work = tempfile::tempdir().unwrap();
    for scenario in scenario::SCENARIOS {
        let mut scripts = Scripts::default();
        for variant in Variant::ALL {
            let (first, second) =
                (scenario.first.solution, scenario.second_solution(variant));
            scripts.set(
                Arm::None,
                variant,
                0,
                Stage::First,
                ScriptedModel::new()
                    .turn(move |t| {
                        t.tool_call("bash", json!({"command": first}))
                    })
                    .turn(|t| t.text("done")),
            );
            scripts.set(
                Arm::None,
                variant,
                0,
                Stage::Second,
                ScriptedModel::new()
                    .turn(move |t| {
                        t.tool_call("bash", json!({"command": second}))
                    })
                    .turn(|t| t.text("done")),
            );
            // Trial 1 does nothing.
            for stage in [Stage::First, Stage::Second] {
                scripts.set(
                    Arm::None,
                    variant,
                    1,
                    stage,
                    ScriptedModel::new().turn(|t| t.text("nothing to do")),
                );
            }
        }
        let mut config =
            config(scenario.name, &Variant::ALL, &[Arm::None], work.path());
        config.trials = 2;
        let report = run(&config, &scripts);
        assert_eq!(report.trials.len(), 4);
        for trial in &report.trials {
            let solved = trial.trial == 0;
            assert_eq!(trial.first.success, Some(solved), "{}", scenario.name);
            assert_eq!(trial.second.success, Some(solved), "{}", scenario.name);
            let expected_stale =
                (trial.variant == Variant::Changed).then_some(false);
            assert_eq!(trial.stale_used, expected_stale);
        }
    }
}

/// The trials the budget lets run: each needs some budget left to start,
/// and one whose first run spends the rest is not finished. A reference
/// for [`evaluate`].
fn expected_trials(limit: f64, costs: &[(f64, f64)]) -> (usize, f64) {
    let mut spent = 0.0;
    for (done, (first, second)) in costs.iter().enumerate() {
        if spent >= limit {
            return (done, spent);
        }
        spent += first;
        if spent >= limit {
            return (done, spent);
        }
        spent += second;
    }
    (costs.len(), spent)
}

#[hegel::test(test_cases = 15)]
fn the_budget_stops_the_evaluation_once_spent(tc: TestCase) {
    let trials = tc.draw(gs::integers::<u32>().min_value(1).max_value(4));
    let cents = || gs::integers::<u32>().min_value(1).max_value(50);
    let costs: Vec<(f64, f64)> = (0..trials)
        .map(|_| {
            (
                f64::from(tc.draw(cents())) / 100.0,
                f64::from(tc.draw(cents())) / 100.0,
            )
        })
        .collect();
    let limit =
        f64::from(tc.draw(gs::integers::<u32>().min_value(1).max_value(200)))
            / 100.0;
    let mut scripts = Scripts::default();
    for (trial, (first, second)) in costs.iter().copied().enumerate() {
        for (stage, cost) in [(Stage::First, first), (Stage::Second, second)] {
            scripts.set(
                Arm::None,
                Variant::Stable,
                trial as u32,
                stage,
                ScriptedModel::new().turn(move |t| t.text("done").cost(cost)),
            );
        }
    }
    let work = tempfile::tempdir().unwrap();
    let mut config =
        config("config-keys", &[Variant::Stable], &[Arm::None], work.path());
    config.trials = trials;
    config.budget = Budget::new(Some(limit));
    let report = run(&config, &scripts);
    let (done, spent) = expected_trials(limit, &costs);
    assert_eq!(report.trials.len(), done);
    assert!(
        (report.spent_usd - spent).abs() < 1e-9,
        "{} {spent}",
        report.spent_usd
    );
    assert_eq!(report.stopped.is_some(), done < costs.len());
    // No run after the stop was started.
    for (trial, _) in costs.iter().enumerate().skip(done + 1) {
        for stage in [Stage::First, Stage::Second] {
            let key = (Arm::None, Variant::Stable, trial as u32, stage);
            assert!(scripts.0[&key].requests().is_empty());
        }
    }
    let costs_seen: Vec<f64> =
        report.trials.iter().map(Trial::cost_usd).collect();
    for (seen, (first, second)) in costs_seen.iter().zip(&costs) {
        assert!((seen - (first + second)).abs() < 1e-9);
    }
}
