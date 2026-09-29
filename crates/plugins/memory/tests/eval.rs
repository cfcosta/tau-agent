//! The retrieval evaluation's own checks: the corpus builds at every
//! level to the same size, near-duplicates and background are what they
//! claim, the metrics are their definitions, and BM25 holds the floors
//! the corpus was written to show.

use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_memory::{
    colbert::{Colbert, Encoder, Tokens},
    eval::{
        self,
        Distance,
        Fixture,
        Hybrid,
        LEVELS,
        Leg,
        Outcome,
        corpus,
        evaluate,
        index_all,
        mrr,
        per_fact,
        recall_at,
        rrf,
    },
    index::{Bm25, words},
    note::slug,
};

fn fixture() -> Fixture {
    eval::fixture().unwrap()
}

#[hegel::test(test_cases = 100)]
fn near_duplicates_are_k_distinct_others_starting_with_the_older(tc: TestCase) {
    let fixture = fixture();
    let fact =
        tc.draw(gs::sampled_from(fixture.facts.clone()).print_as_debug());
    let k = tc.draw(gs::integers::<usize>().max_value(per_fact() - 1));
    let near = fact.near_duplicates(k);
    assert_eq!(near.len(), k);
    let distinct: BTreeSet<_> = near.iter().collect();
    assert_eq!(distinct.len(), k, "{near:?}");
    assert!(!near.contains(&(fact.subject.clone(), fact.value.clone())));
    if k > 0 {
        assert_eq!(near[0], (fact.subject.clone(), fact.old_value.clone()));
    }
    // A level's near-duplicates are the next level's first ones.
    assert_eq!(fact.near_duplicates(k + 1)[..k], near[..]);
}

#[hegel::test(test_cases = 100)]
fn the_background_grows_by_extension(tc: TestCase) {
    let fixture = fixture();
    let most = fixture.facts.len() * (per_fact() - 1);
    let n = tc.draw(gs::integers::<usize>().max_value(most));
    let m = tc.draw(gs::integers::<usize>().min_value(n).max_value(most));
    let (short, long) = (
        fixture.background(n).unwrap(),
        fixture.background(m).unwrap(),
    );
    assert_eq!(short.len(), n);
    assert_eq!(long[..n], short[..]);
    let titles: BTreeSet<String> =
        long.iter().map(|draft| slug(&draft.title)).collect();
    assert_eq!(titles.len(), m, "background ids collide");
}

/// Every level holds the same number of notes; every query's answer is
/// a current note, and its older version, from level 1, a superseded one.
#[test]
fn every_level_builds_to_one_size() {
    let fixture = fixture();
    for k in LEVELS {
        level_builds_to_one_size(&fixture, k);
    }
}

fn level_builds_to_one_size(fixture: &Fixture, k: usize) {
    let dir = tempfile::tempdir().unwrap();
    let corpus = corpus(dir.path(), fixture, k).unwrap();
    assert_eq!(corpus.notes.len(), fixture.facts.len() * per_fact());
    assert!(corpus.notes.unreadable().is_empty());

    let linked = fixture.facts.iter().filter(|fact| fact.link.is_some());
    let count = |distance| {
        corpus
            .queries
            .iter()
            .filter(|query| query.distance == distance)
            .count()
    };
    assert_eq!(count(Distance::Verbatim), fixture.facts.len());
    assert_eq!(count(Distance::Paraphrase), fixture.facts.len());
    assert_eq!(count(Distance::Indirect), linked.count());
    for query in &corpus.queries {
        let answer = corpus.notes.get(&query.answer).unwrap();
        assert!(!answer.is_superseded(), "{}", query.answer);
        match &query.older {
            Some(older) => {
                assert!(k > 0);
                assert!(corpus.notes.get(older).unwrap().is_superseded());
            }
            None => assert_eq!(k, 0),
        }
    }
}

fn outcome(tc: &TestCase) -> Outcome {
    Outcome {
        distance: Distance::Verbatim,
        rank: tc.draw(gs::optional(
            gs::integers::<usize>().min_value(1).max_value(eval::DEPTH),
        )),
        older_first: None,
    }
}

#[hegel::test(test_cases = 200)]
fn the_metrics_are_their_definitions(tc: TestCase) {
    let n = tc.draw(gs::integers::<usize>().max_value(30));
    let outcomes: Vec<Outcome> = (0..n).map(|_| outcome(&tc)).collect();
    let cutoff = tc.draw(gs::integers::<usize>().max_value(eval::DEPTH));
    let found = outcomes
        .iter()
        .filter(|o| matches!(o.rank, Some(r) if r <= cutoff))
        .count();
    let want = if n == 0 { 0.0 } else { found as f32 / n as f32 };
    assert!((recall_at(&outcomes, cutoff) - want).abs() < 1e-6);
    // A deeper cutoff never finds less; MRR sits under recall at the
    // deepest one.
    assert!(recall_at(&outcomes, cutoff + 1) >= recall_at(&outcomes, cutoff));
    assert!(mrr(&outcomes) <= recall_at(&outcomes, eval::DEPTH) + 1e-6);
    assert!(mrr(&outcomes) >= 0.0);
}

/// BM25 finds what shares its words, and the hop reaches what only a
/// link leads to: the floors the corpus was written to show.
#[test]
fn bm25_holds_its_floors() {
    let fixture = fixture();
    for k in [0, 16] {
        let dir = tempfile::tempdir().unwrap();
        let corpus = corpus(dir.path(), &fixture, k).unwrap();
        let mut index = Bm25::new();
        index_all(&mut index, &corpus.notes).unwrap();
        let of = |outcomes: &[Outcome], distance| -> Vec<Outcome> {
            outcomes
                .iter()
                .filter(|o| o.distance == distance)
                .copied()
                .collect()
        };
        let alone = evaluate(&corpus, &index, false).unwrap();
        let hops = evaluate(&corpus, &index, true).unwrap();
        assert!(
            recall_at(&of(&alone, Distance::Verbatim), 10) >= 0.9,
            "k={k}"
        );
        assert_eq!(
            recall_at(&of(&hops, Distance::Indirect), 10),
            1.0,
            "k={k}: every indirect answer is one hop from a verbatim match"
        );
        // Superseded versions are weighed down below their replacement.
        assert!(alone.iter().all(|o| o.older_first != Some(true)), "k={k}");
    }
}

/// Encodes each word as a fixed unit vector, so the ColBERT legs run
/// without a model.
#[derive(Clone)]
struct Fake;

fn vector(word: &str) -> Vec<f32> {
    let mut seed = word
        .bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    let raw: Vec<f32> = (0..8)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
        })
        .collect();
    let norm = raw.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-6);
    raw.into_iter().map(|x| x / norm).collect()
}

impl Encoder for Fake {
    fn documents(&mut self, texts: &[String]) -> anyhow::Result<Vec<Tokens>> {
        Ok(texts.iter().map(|text| self.query(text).unwrap()).collect())
    }

    fn query(&mut self, text: &str) -> anyhow::Result<Tokens> {
        Ok(Tokens {
            dim: 8,
            values: words(text).flat_map(|word| vector(&word)).collect(),
        })
    }

    fn model(&self) -> &str {
        "fake"
    }
}

/// The whole run, with every leg: a row per leg, level, hop setting and
/// distance, each over the queries it should count.
#[test]
fn a_run_reports_every_leg_at_every_level() {
    let fixture = fixture();
    let work = tempfile::tempdir().unwrap();
    let legs = [
        Leg::new("bm25", || Box::new(Bm25::new())),
        Leg::new("colbert", || Box::new(Colbert::new(Fake))),
        Leg::new("hybrid", || Box::new(Hybrid::new(Colbert::new(Fake)))),
    ];
    let mut seen = Vec::new();
    let report =
        eval::run(&fixture, &legs, work.path(), |k| seen.push(k)).unwrap();
    assert_eq!(seen, LEVELS);
    assert_eq!(report.len(), legs.len() * LEVELS.len() * 2 * 3);
    for row in &report {
        assert!((0.0..=1.0).contains(&row.recall_5));
        assert!(row.recall_5 <= row.recall_10);
        assert!(row.queries > 0);
    }
    // The work directory is left empty.
    assert_eq!(std::fs::read_dir(work.path()).unwrap().count(), 0);
    let table = eval::table(&report);
    for name in ["bm25", "colbert", "hybrid"] {
        assert!(table.contains(name), "{table}");
    }
}

/// A ranking: distinct ids, best first.
#[hegel::composite]
fn ranking(tc: &TestCase) -> Vec<String> {
    let mut ids = tc.draw(gs::permutations(vec!["a", "b", "c", "d", "e"]));
    ids.truncate(tc.draw(gs::integers::<usize>().max_value(5)));
    ids.into_iter().map(str::to_owned).collect()
}

fn order(fused: &[(String, f32)]) -> Vec<&str> {
    fused.iter().map(|(id, _)| id.as_str()).collect()
}

/// Fusion as laws, not its formula: each id listed anywhere once, best
/// first with ties by id; the order of the lists does not matter; one
/// list fuses to itself; an id first everywhere is first; and a list
/// more raises its own ids and nothing else.
/// Three ids at the same ranks, in turn: a true tie, whatever order
/// the sums are taken in.
fn rotations() -> Vec<Vec<String>> {
    [["a", "b", "c"], ["c", "a", "b"], ["b", "c", "a"]]
        .iter()
        .map(|list| list.iter().map(|id| (*id).to_owned()).collect())
        .collect()
}

#[hegel::test(test_cases = 300)]
#[hegel::explicit_test_case(
    lists = rotations(),
    shuffled = rotations(),
    first = "a",
    extra = Vec::<String>::new()
)]
fn rrf_fuses_by_rank(tc: TestCase) {
    let lists: Vec<Vec<String>> =
        tc.draw(gs::vecs(ranking()).min_size(1).max_size(4));
    let fused = rrf(&lists);
    let listed: BTreeSet<&String> = lists.iter().flatten().collect();
    assert_eq!(fused.len(), listed.len());
    assert!(
        fused
            .iter()
            .all(|(id, score)| listed.contains(id) && *score > 0.0)
    );
    for pair in fused.windows(2) {
        let ((a, x), (b, y)) = (&pair[0], &pair[1]);
        assert!(x > y || (x == y && a < b), "{fused:?}");
    }
    // Ids at the same ranks, in whichever lists, tie.
    let ranks = |id: &str| -> Vec<usize> {
        let mut ranks: Vec<usize> = lists
            .iter()
            .filter_map(|list| list.iter().position(|other| other == id))
            .collect();
        ranks.sort();
        ranks
    };
    for (a, x) in &fused {
        for (b, y) in &fused {
            if ranks(a) == ranks(b) {
                assert_eq!(x, y, "{a} and {b} in {lists:?}");
            }
        }
    }

    let shuffled = tc.draw(gs::permutations(lists.clone()));
    assert_eq!(order(&rrf(&shuffled)), order(&fused), "{shuffled:?}");

    let alone = rrf(&lists[..1]);
    assert_eq!(order(&alone), lists[0]);

    let first = tc.draw(gs::sampled_from(vec!["a", "b", "c"]));
    let led: Vec<Vec<String>> = lists
        .iter()
        .map(|list| {
            let mut list: Vec<String> =
                list.iter().filter(|id| *id != first).cloned().collect();
            list.insert(0, first.to_owned());
            list
        })
        .collect();
    assert_eq!(rrf(&led)[0].0, first, "{led:?}");

    let extra = tc.draw(ranking());
    let more: BTreeMap<String, f32> =
        rrf(&[lists.clone(), vec![extra.clone()]].concat())
            .into_iter()
            .collect();
    let before: BTreeMap<String, f32> = fused.into_iter().collect();
    for (id, score) in &more {
        match before.get(id) {
            Some(was) if extra.contains(id) => assert!(score > was, "{id}"),
            Some(was) => assert_eq!(score, was, "{id}"),
            None => assert!(extra.contains(id), "{id}"),
        }
    }
}

/// MRR is the mean of one over each answer's rank, a missed answer
/// counting nothing.
#[hegel::test(test_cases = 200)]
fn mrr_is_the_mean_reciprocal_rank(tc: TestCase) {
    let n = tc.draw(gs::integers::<usize>().min_value(1).max_value(30));
    let outcomes: Vec<Outcome> = (0..n).map(|_| outcome(&tc)).collect();
    let reciprocal =
        |outcome: &Outcome| outcome.rank.map_or(0.0, |r| 1.0 / r as f64);
    let want = outcomes.iter().map(reciprocal).sum::<f64>() / n as f64;
    assert!(
        (f64::from(mrr(&outcomes)) - want).abs() < 1e-5,
        "{outcomes:?}"
    );
    // One answer more, found first, never lowers it.
    let mut better = outcomes.clone();
    better.push(Outcome {
        rank: Some(1),
        ..outcomes[0]
    });
    assert!(mrr(&better) >= mrr(&outcomes) - 1e-6);
}
