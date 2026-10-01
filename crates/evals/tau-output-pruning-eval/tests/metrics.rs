//! The summaries against a reference: sums, recall and means per
//! workload and in total.

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_output_pruning_eval::{
    metrics::{Trial, summarize},
    runner::Bands,
    workload::Kind,
};

#[hegel::composite]
fn trial(tc: &TestCase) -> Trial {
    let needles = tc.draw(gs::integers::<usize>().max_value(5));
    let retained = tc.draw(gs::integers::<usize>().max_value(needles));
    let tail_retained = tc.draw(gs::integers::<usize>().max_value(needles));
    let tokens_before = tc.draw(gs::integers::<usize>().max_value(100_000));
    let tokens_after = tc.draw(gs::integers::<usize>().max_value(120_000));
    let tokens_full = tc.draw(gs::integers::<usize>().max_value(200_000));
    Trial {
        workload: tc
            .draw(gs::sampled_from(Kind::ALL.to_vec()).print_as_debug()),
        seed: tc.draw(gs::integers::<u64>().max_value(9)),
        needles,
        retained,
        tail_retained,
        missed: Vec::new(),
        lines: 1,
        kept_lines: None,
        chunks: None,
        kept_chunks: None,
        spilled: tc.draw(gs::booleans()),
        tokens_full,
        tokens_before,
        tokens_after,
        replaced: tc.draw(gs::booleans()),
        requests: tc.draw(gs::integers::<usize>().max_value(12)),
        jev_input_tokens: 0,
        cost_usd: tc.draw(gs::integers::<u32>().max_value(10_000)) as f64 / 1e6,
        answers: Bands {
            noise: tc.draw(gs::integers::<usize>().max_value(200)),
            uncertain: tc.draw(gs::integers::<usize>().max_value(200)),
            needed: tc.draw(gs::integers::<usize>().max_value(200)),
        },
        scores: Vec::new(),
        chunk_tokens: Vec::new(),
        needle_chunks: Vec::new(),
        latency_ms: tc.draw(gs::integers::<u64>().max_value(10_000)),
        archive: None,
        error: tc
            .draw(gs::booleans())
            .then(|| "Jev answered with status 503".to_owned()),
    }
}

/// What a summary of `trials` should say, computed plainly.
/// A trial's share of `of` it saved, or 0.
fn saved(of: usize, after: usize) -> f64 {
    if of == 0 || after >= of {
        0.0
    } else {
        (of - after) as f64 / of as f64
    }
}

fn reference(trials: &[&Trial]) -> (usize, usize, usize, usize, f64, f64, f64) {
    let mut needles = 0;
    let mut retained = 0;
    let mut requests = 0;
    let mut replaced = 0;
    let mut reductions = 0.0;
    let mut of_whole = 0.0;
    let mut cost = 0.0;
    for trial in trials {
        needles += trial.needles;
        retained += trial.retained;
        requests += trial.requests;
        replaced += usize::from(trial.replaced);
        reductions += saved(trial.tokens_before, trial.tokens_after);
        of_whole += saved(trial.tokens_full, trial.tokens_after);
        cost += trial.cost_usd;
    }
    let mean = |total: f64| {
        if trials.is_empty() {
            0.0
        } else {
            total / trials.len() as f64
        }
    };
    (
        needles,
        retained,
        requests,
        replaced,
        mean(reductions),
        mean(of_whole),
        cost,
    )
}

/// One summary per workload with trials, in order, then the total;
/// each matches the reference, and recall is retained over needed, or
/// none when nothing was needed.
#[hegel::test(test_cases = 200)]
fn summaries_match_a_reference(tc: TestCase) {
    let trials: Vec<Trial> =
        tc.draw(gs::vecs(trial()).max_size(30).print_as_debug());
    let summaries = summarize(&trials);
    let present: Vec<Kind> = Kind::ALL
        .into_iter()
        .filter(|kind| trials.iter().any(|trial| trial.workload == *kind))
        .collect();
    assert_eq!(summaries.len(), present.len() + 1);
    let groups = present
        .iter()
        .map(|kind| {
            (
                kind.name(),
                trials.iter().filter(|t| t.workload == *kind).collect(),
            )
        })
        .chain([("total", trials.iter().collect::<Vec<_>>())]);
    for (summary, (name, of)) in summaries.iter().zip(groups) {
        let of: Vec<&Trial> = of;
        let (needles, retained, requests, replaced, mean, mean_full, cost) =
            reference(&of);
        assert_eq!(summary.workload, name);
        assert_eq!(summary.trials, of.len());
        assert_eq!(summary.needles, needles);
        assert_eq!(summary.retained, retained);
        assert_eq!(summary.requests, requests);
        assert_eq!(summary.replaced, replaced);
        assert_eq!(
            summary.answers,
            of.iter().fold(Bands::default(), |sum, t| Bands {
                noise: sum.noise + t.answers.noise,
                uncertain: sum.uncertain + t.answers.uncertain,
                needed: sum.needed + t.answers.needed,
            })
        );
        assert_eq!(
            summary.errors,
            of.iter().filter(|t| t.error.is_some()).count()
        );
        assert!((summary.mean_reduction - mean).abs() < 1e-9);
        assert!((summary.mean_reduction_full - mean_full).abs() < 1e-9);
        assert!((summary.cost_usd - cost).abs() < 1e-9);
        match summary.recall() {
            None => assert_eq!(needles, 0),
            Some(recall) => {
                assert!(
                    (recall - retained as f64 / needles as f64).abs() < 1e-12
                );
                assert!((0.0..=1.0).contains(&recall));
            }
        }
    }
}
