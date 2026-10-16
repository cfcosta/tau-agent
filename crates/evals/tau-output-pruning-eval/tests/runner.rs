//! The runner against fake Jevs: one that answers from ground truth,
//! one that keeps nothing, one that fails. No network.

use std::{collections::HashSet, sync::Arc};

use tau_fast_compaction::{
    OutputPruning,
    Settings,
    output::{self, Line},
    state::estimate_tokens,
};
use tau_jev::{Answer, Jev, JevError, fake::FakeJev};
use tau_output_pruning_eval::{
    metrics::summarize,
    runner::{self, Config, Exec, evaluate, exec_result},
    workload::{Kind, Workload, generate},
};
use tau_testing::block_on;

/// Keeps exactly the chunks holding a needle line.
fn ground_truth(workload: &Workload) -> FakeJev {
    let needles: HashSet<String> =
        workload.needles.iter().map(|n| n.text.clone()).collect();
    FakeJev::new(move |request| {
        let chunks = request.state["chunks"].as_array().unwrap();
        let answers = request
            .questions
            .keys()
            .map(|id| {
                let text = chunks
                    .iter()
                    .find(|chunk| chunk["id"] == id.as_str())
                    .unwrap()["text"]
                    .as_str()
                    .unwrap();
                let needed =
                    text.split('\n').any(|line| needles.contains(line));
                (
                    id.clone(),
                    Answer::Noul {
                        noul: if needed { 0.95 } else { 0.0 },
                    },
                )
            })
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    })
}

/// What pruning renders when the chunks `keep` says stay, with the
/// archive at `archive`.
fn rendered(
    output: &str,
    archive: &str,
    keep: impl Fn(usize, &[Line<'_>]) -> bool,
) -> String {
    let lines = output::lines(output);
    let chunks =
        output::chunks(lines.len(), OutputPruning::default().chunk_lines);
    let last = chunks.len() - 1;
    let kept: Vec<bool> = chunks
        .iter()
        .enumerate()
        .map(|(index, range)| {
            index == 0 || index == last || keep(index, &lines[range.clone()])
        })
        .collect();
    output::render(&lines, &chunks, &kept, archive)
}

fn trial(
    workload: &Workload,
    jev: FakeJev,
    exec: Exec,
) -> (tau_output_pruning_eval::metrics::Trial, tempfile::TempDir) {
    let work = tempfile::tempdir().unwrap();
    let trial = block_on(runner::run(
        workload,
        Arc::new(jev) as Arc<dyn Jev>,
        Settings::default(),
        work.path(),
        exec,
    ))
    .unwrap();
    (trial, work)
}

/// With a Jev that keeps exactly the needle chunks, every needle
/// reaches the model and the result is exactly the first, last and
/// needle chunks: the reduction is that render's against what
/// `exec_command` showed. The archive holds what the tool returned:
/// the whole output, or its tail, in which a cut-away needle is lost.
#[test]
fn ground_truth_keeps_every_needle() {
    for exec in [Exec::Tau, Exec::Whole] {
        for kind in Kind::ALL {
            for seed in 0..2 {
                let workload = generate(kind, seed);
                let jev = ground_truth(&workload);
                let (trial, _work) = trial(&workload, jev.clone(), exec);
                let context = format!("{kind:?} #{seed} {exec:?}: {trial:?}");
                assert_eq!(trial.error, None, "{context}");
                let (native, cut) = match exec {
                    Exec::Tau => exec_result(&workload.output),
                    Exec::Whole => (workload.output.clone(), false),
                };
                assert_eq!(trial.cut, cut, "{context}");
                let tail: HashSet<&str> = native.split('\n').collect();
                let seen = workload
                    .needles
                    .iter()
                    .filter(|needle| tail.contains(needle.text.as_str()))
                    .count();
                assert_eq!(trial.retained, seen, "{context}");
                assert_eq!(trial.missed.len(), trial.needles - seen);
                assert!(trial.replaced, "{context}");
                let archive = trial.archive.clone().unwrap();
                assert_eq!(std::fs::read_to_string(&archive).unwrap(), native);
                let needles: HashSet<&str> =
                    workload.needles.iter().map(|n| n.text.as_str()).collect();
                let expected = rendered(&native, &archive, |_, lines| {
                    lines.iter().any(|line| needles.contains(line.text))
                });
                assert_eq!(trial.tokens_before, estimate_tokens(&native));
                assert_eq!(
                    trial.tokens_after,
                    estimate_tokens(&expected),
                    "{context}"
                );
                let reduction = 1.0
                    - estimate_tokens(&expected) as f64
                        / estimate_tokens(&native) as f64;
                assert!((trial.reduction() - reduction).abs() < 1e-12);
                assert!(trial.reduction() > 0.5, "{context}: {reduction}");
                assert!(trial.requests > 0 && trial.cost_usd > 0.0);
                // The recorded answers replay the decision: each needle's
                // chunk scored as needed, every other asked chunk not.
                assert_eq!(trial.needle_chunks.len(), seen);
                assert_eq!(Some(trial.scores.len()), trial.chunks);
                for (index, score) in trial.scores.iter().enumerate() {
                    let needed = trial.needle_chunks.contains(&index);
                    let ends = index == 0 || index + 1 == trial.scores.len();
                    assert_eq!(
                        *score,
                        (!ends).then_some(if needed { 0.95 } else { 0.0 }),
                        "{context}: chunk {index}"
                    );
                }
                if kind == Kind::EarlierRequirement {
                    // The history came in segments: only some states
                    // held the requirement, and the needle was kept.
                    let holding = jev
                        .requests()
                        .iter()
                        .filter(|request| {
                            request.state["history"]
                                .to_string()
                                .contains("IMPORTANT")
                        })
                        .count();
                    assert!(
                        holding > 0 && holding < trial.requests,
                        "{context}"
                    );
                }
            }
        }
    }
}

/// With a Jev that keeps nothing, the first and last chunks still stay:
/// recall counts exactly the needles in them.
#[test]
fn keeping_nothing_keeps_the_ends() {
    for kind in Kind::ALL {
        let workload = generate(kind, 3);
        let (trial, _work) =
            trial(&workload, FakeJev::nouls(|_| 0.0), Exec::Whole);
        let lines = output::lines(&workload.output);
        let chunks =
            output::chunks(lines.len(), OutputPruning::default().chunk_lines);
        let (first, last) = (&chunks[0], &chunks[chunks.len() - 1]);
        let at_the_ends = workload
            .needles
            .iter()
            .filter(|n| first.contains(&n.line) || last.contains(&n.line))
            .count();
        assert!(trial.replaced, "{kind:?}");
        assert_eq!(trial.retained, at_the_ends, "{kind:?}");
        assert_eq!(trial.kept_chunks, Some(2));
        assert_eq!((trial.answers.uncertain, trial.answers.needed), (0, 0));
        assert!(trial.answers.noise >= chunks.len() - 2);
        let expected = rendered(
            &workload.output,
            trial.archive.as_deref().unwrap(),
            |_, _| false,
        );
        assert_eq!(
            trial.tokens_after,
            estimate_tokens(&expected),
            "{kind:?}: {trial:?}"
        );
    }
}

/// Jev's answers decide against the threshold alone: a Jev uncertain
/// about every chunk (0.3) lets every asked chunk go, and one at the
/// threshold (0.5) keeps them all, so nothing is replaced.
#[test]
fn the_threshold_alone_decides() {
    for kind in [Kind::AllNoise, Kind::NeedleError] {
        let workload = generate(kind, 2);
        let (uncertain, _work) =
            trial(&workload, FakeJev::nouls(|_| 0.3), Exec::Tau);
        assert!(uncertain.replaced, "{kind:?}");
        assert_eq!(uncertain.kept_chunks, Some(2));
        assert_eq!((uncertain.answers.noise, uncertain.answers.needed), (0, 0));
        assert!(uncertain.answers.uncertain > 0);

        let (needed, _work) =
            trial(&workload, FakeJev::nouls(|_| 0.5), Exec::Tau);
        assert!(!needed.replaced, "{kind:?}");
        assert_eq!(needed.retained, needed.tail_retained);
        assert_eq!(needed.kept_chunks, needed.chunks);
        assert_eq!((needed.answers.noise, needed.answers.uncertain), (0, 0));
        assert!(needed.answers.needed > 0);
    }
}

/// A Jev that fails leaves the result as `exec_command` returned it: the
/// whole output keeps every needle with no reduction; a cut one keeps
/// what its tail held.
#[test]
fn a_failing_jev_leaves_the_output() {
    for kind in Kind::ALL {
        let workload = generate(kind, 1);
        let failing = || FakeJev::new(|_| Err(JevError::Status(503)));
        let (whole, _work) = trial(&workload, failing(), Exec::Whole);
        assert!(!whole.replaced);
        assert_eq!(whole.retained, whole.needles);
        assert_eq!(whole.reduction(), 0.0);
        assert_eq!(whole.tokens_after, whole.tokens_before);
        assert_eq!(
            whole.error.as_deref(),
            Some("Jev answered with status 503")
        );
        assert_eq!(whole.cost_usd, 0.0);

        let (cut, _work) = trial(&workload, failing(), Exec::Tau);
        assert!(!cut.replaced);
        assert_eq!(cut.retained, cut.tail_retained);
        assert_eq!(cut.reduction(), 0.0);
    }
    let middle = generate(Kind::CutMiddle, 1);
    let (trial, _work) = trial(
        &middle,
        FakeJev::new(|_| Err(JevError::Status(503))),
        Exec::Tau,
    );
    assert_eq!((trial.retained, trial.tail_retained), (0, 0));
}

/// The evaluation runs every kind per seed, sums what it spent, and
/// stops once the budget is passed.
#[test]
fn the_budget_stops_the_evaluation() {
    let work = tempfile::tempdir().unwrap();
    let mut config = Config::new(work.path().to_owned());
    config.seeds = vec![0, 1];
    config.kinds = vec![Kind::NeedleError, Kind::AllNoise];
    let jev: Arc<dyn Jev> = Arc::new(FakeJev::nouls(|_| 0.0));
    let mut seen = 0;
    let report =
        block_on(evaluate(&config, jev.clone(), &mut |_| seen += 1)).unwrap();
    assert_eq!(report.trials.len(), 4);
    assert_eq!(seen, 4);
    assert_eq!(report.stopped, None);
    let spent: f64 = report.trials.iter().map(|t| t.cost_usd).sum();
    assert!((report.spent_usd - spent).abs() < 1e-12);
    assert_eq!(report.summaries, summarize(&report.trials));

    let work = tempfile::tempdir().unwrap();
    config.work = work.path().to_owned();
    config.budget_usd = Some(report.trials[0].cost_usd / 2.0);
    let report = block_on(evaluate(&config, jev, &mut |_| {})).unwrap();
    assert_eq!(report.trials.len(), 1);
    assert!(report.stopped.is_some());
}
