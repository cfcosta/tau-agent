//! Search and recall: BM25 against its formula written out plainly, and
//! recall's budget, link hops and ranking as laws over random notes.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::pool_note;
use hegel::{TestCase, generators as gs};
use tau_memory_host::{
    index::{B, Bm25, Index, IndexError, K1, index_text},
    note::{By, Note, NoteType, Source},
    recall::{SUPERSEDED_WEIGHT, linked, recall},
    store::Notes,
};

const VOCAB: [&str; 8] = [
    "retry", "after", "header", "jitter", "cache", "lane", "delta", "fork",
];

#[hegel::composite]
fn words_text(tc: &TestCase) -> String {
    tc.draw(gs::vecs(gs::sampled_from(VOCAB.to_vec())).max_size(12))
        .join(" ")
}

/// BM25 the plain way, in f64: every term of the query, every document.
fn reference(
    docs: &BTreeMap<String, String>,
    query: &str,
) -> Vec<(String, f64)> {
    let tokens: BTreeMap<&String, Vec<String>> = docs
        .iter()
        .map(|(id, text)| (id, tau_memory_host::index::words(text).collect()))
        .collect();
    let n = docs.len() as f64;
    let avg = tokens.values().map(Vec::len).sum::<usize>() as f64 / n;
    let mut terms: Vec<String> = tau_memory_host::index::words(query).collect();
    terms.sort();
    terms.dedup();
    let mut scores: Vec<(String, f64)> = tokens
        .iter()
        .map(|(id, doc)| {
            let score = terms
                .iter()
                .map(|term| {
                    let tf = doc.iter().filter(|w| *w == term).count() as f64;
                    if tf == 0.0 {
                        return 0.0;
                    }
                    let df =
                        tokens.values().filter(|d| d.contains(term)).count()
                            as f64;
                    let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                    let norm = f64::from(K1)
                        * (1.0 - f64::from(B)
                            + f64::from(B) * doc.len() as f64 / avg.max(1.0));
                    idf * tf * (f64::from(K1) + 1.0) / (tf + norm)
                })
                .sum();
            ((*id).clone(), score)
        })
        .filter(|(_, score)| *score > 0.0)
        .collect();
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scores
}

#[hegel::test(test_cases = 200)]
fn bm25_scores_as_its_formula(tc: TestCase) {
    let mut index = Bm25::new();
    let mut docs = BTreeMap::new();
    // Documents written, some rewritten and some removed, as notes are.
    for _ in 0..tc.draw(gs::integers::<usize>().max_value(12)) {
        let id = tc
            .draw(gs::sampled_from(vec!["a", "b", "c", "d", "e", "f"]))
            .to_owned();
        if tc.draw(gs::booleans()) || !docs.contains_key(&id) {
            let text = tc.draw(words_text());
            index.upsert(&id, &text).unwrap();
            docs.insert(id, text);
        } else {
            index.remove(&id).unwrap();
            docs.remove(&id);
        }
    }
    let query = tc.draw(words_text());
    let got = index.search(&query, 100).unwrap();
    let want = if docs.is_empty() {
        Vec::new()
    } else {
        reference(&docs, &query)
    };
    assert_eq!(got.len(), want.len(), "{got:?} vs {want:?}");
    for ((id, score), (want_id, want_score)) in got.iter().zip(&want) {
        let close = (f64::from(*score) - want_score).abs()
            <= 1e-4 * want_score.max(1.0);
        assert!(close, "{id}: {score} vs {want_id}: {want_score}");
    }
    // Same order, where scores differ by more than rounding.
    for pair in want.windows(2) {
        if pair[0].1 - pair[1].1 > 1e-3 {
            let at = |id: &str| got.iter().position(|(g, _)| g == id).unwrap();
            assert!(at(&pair[0].0) < at(&pair[1].0), "{got:?} vs {want:?}");
        }
    }
}

/// Notes from the pool, with bodies drawn from a small vocabulary, in a
/// store and an index.
fn setup(tc: &TestCase, dir: &std::path::Path) -> (Notes, Bm25) {
    let mut notes = Notes::open(dir).unwrap();
    let mut index = Bm25::new();
    // Sometimes no note links anywhere, so seeds are left to fill the
    // budget.
    let unlinked = tc.draw(gs::weighted_booleans(0.2));
    for _ in 0..tc.draw(gs::integers::<usize>().min_value(1).max_value(8)) {
        let mut note: Note = tc.draw(pool_note());
        note.body = format!("{}\n{}", note.body, tc.draw(words_text()));
        if unlinked {
            note.links.clear();
            note.body = tc.draw(words_text());
        }
        note.valid_to = tc.draw(gs::optional(gs::just(5_u64)));
        if notes.create(note.clone()).is_ok() {
            index.upsert(&note.id, &index_text(&note)).unwrap();
        }
    }
    (notes, index)
}

/// The seeds recall starts from: what the index answers for `limit`,
/// notes that are gone left out, superseded ones weighed down, best
/// first and ties by id.
fn weighted(
    notes: &Notes,
    index: &dyn Index,
    query: &str,
    limit: usize,
) -> Vec<String> {
    let mut seeds: Vec<(String, f32)> = index
        .search(query, limit)
        .unwrap()
        .into_iter()
        .filter_map(|(id, score)| {
            let note = notes.get(&id)?;
            let score = match (note.is_superseded(), score >= 0.0) {
                (false, _) => score,
                (true, true) => score * SUPERSEDED_WEIGHT,
                (true, false) => score / SUPERSEDED_WEIGHT,
            };
            Some((id, score))
        })
        .collect();
    seeds.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    seeds.into_iter().map(|(id, _)| id).collect()
}

/// Recall keeps its budget and lists each note once; seeds come in
/// their weighted order, and each linked note after the seed it hangs
/// from; and the budget fills when there is enough to fill it with.
#[hegel::test(test_cases = 200)]
fn recall_keeps_its_budget_and_follows_links(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let (notes, index) = setup(&tc, dir.path());
    let query = tc.draw(words_text());
    let budget = tc.draw(hegel::one_of!(
        gs::integers::<usize>().max_value(3),
        gs::integers::<usize>().max_value(6),
    ));
    let hits = recall(&notes, &index, &query, budget).unwrap();

    assert!(hits.len() <= budget);
    let ids: Vec<&str> = hits.iter().map(|hit| hit.id.as_str()).collect();
    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), ids.len(), "each note once: {ids:?}");
    // Recall asks the index for three times its budget.
    let seeds = weighted(&notes, &index, &query, budget * 3);
    let mut last_seed = None;
    for (at, hit) in hits.iter().enumerate() {
        let note = notes.get(&hit.id).expect("a hit is a note");
        assert_eq!(hit.superseded, note.is_superseded());
        match &hit.via {
            // A seed, after the seeds that outrank it.
            None => {
                let rank = seeds
                    .iter()
                    .position(|id| *id == hit.id)
                    .expect("search found it");
                assert!(last_seed < Some(rank), "{ids:?} vs {seeds:?}");
                last_seed = Some(rank);
            }
            // Reached by a link from a seed listed before it.
            Some((from, kind)) => {
                tc.event("hop used");
                let seed = ids
                    .iter()
                    .position(|id| id == from)
                    .expect("its seed is listed");
                assert!(seed < at && hits[seed].via.is_none());
                assert!(
                    linked(&notes, from).contains(&(hit.id.clone(), *kind))
                );
            }
        }
    }
    // A third of the budget is kept for linked notes, from the seeds
    // that come first; whatever they leave, later seeds fill.
    let first = seeds.len().min(budget - budget / 3);
    let mut candidates: BTreeSet<&str> =
        seeds.iter().map(String::as_str).collect();
    for seed in &seeds[..first] {
        candidates.extend(
            linked(&notes, seed)
                .into_iter()
                .map(|(id, _)| notes.get(&id).unwrap().id.as_str()),
        );
    }
    assert_eq!(hits.len(), budget.min(candidates.len()), "{hits:?}");
    if hits.iter().filter(|hit| hit.via.is_none()).count() > first {
        tc.event("reserve left unused");
    }
}

/// An index that answers every query with the same drawn scores, which
/// may be negative, as MaxSim's can: the dot products of unit vectors.
struct Scored(Vec<(String, f32)>);

impl Index for Scored {
    fn upsert(&mut self, _: &str, _: &str) -> Result<(), IndexError> {
        Ok(())
    }

    fn remove(&mut self, _: &str) -> Result<(), IndexError> {
        Ok(())
    }

    fn search(
        &self,
        _: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, IndexError> {
        let mut hits = self.0.clone();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hits.truncate(limit);
        Ok(hits)
    }
}

/// The seeds recall lists for `query`, in order.
fn seeds_of(notes: &Notes, index: &dyn Index, query: &str) -> Vec<String> {
    recall(notes, index, query, 12)
        .unwrap()
        .into_iter()
        .filter(|hit| hit.via.is_none())
        .map(|hit| hit.id)
        .collect()
}

/// Supersedes `id`, and checks it did not rise among `index`'s seeds,
/// and the others kept their order.
fn supersede_and_check(
    notes: &mut Notes,
    index: &dyn Index,
    query: &str,
    id: &str,
) {
    let before = seeds_of(notes, index, query);
    let mut note = notes.get(id).unwrap().clone();
    note.valid_to = Some(9);
    notes.update(note).unwrap();
    let after = seeds_of(notes, index, query);
    let at = |list: &[String]| list.iter().position(|x| x == id).unwrap();
    assert!(at(&after) >= at(&before), "{before:?} then {after:?}");
    let others = |list: &[String]| -> Vec<String> {
        list.iter().filter(|x| *x != id).cloned().collect()
    };
    assert_eq!(others(&after), others(&before), "{id}");
}

/// Superseding a note never raises it, and leaves the others in their
/// order, under BM25 or an index whose scores may be negative.
#[hegel::test(test_cases = 200)]
fn superseding_a_note_never_raises_it(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let (mut notes, bm25) = setup(&tc, dir.path());
    let query = tc.draw(words_text());
    let index: Box<dyn Index> = if tc.draw(gs::booleans()) {
        tc.event("bm25");
        Box::new(bm25)
    } else {
        tc.event("scored");
        let scores = notes
            .iter()
            .map(|note| {
                let score =
                    tc.draw(gs::floats::<f32>().min_value(-5.0).max_value(5.0));
                (note.id.clone(), score)
            })
            .collect();
        Box::new(Scored(scores))
    };
    // Any current seed: the first cannot rise, so it proves nothing alone.
    let current: Vec<String> = seeds_of(&notes, index.as_ref(), &query)
        .into_iter()
        .filter(|id| !notes.get(id).unwrap().is_superseded())
        .collect();
    tc.assume(!current.is_empty());
    let id = tc.draw(gs::sampled_from(current));
    supersede_and_check(&mut notes, index.as_ref(), &query, &id);
}

/// A note with nothing but an id and a description.
fn plain(id: &str, description: &str) -> Note {
    Note {
        id: id.into(),
        title: format!("about {id}"),
        description: description.into(),
        kind: NoteType::Fact,
        tags: Vec::new(),
        created: 1,
        updated: 1,
        valid_from: 1,
        valid_to: None,
        stale: None,
        source: Source::new(By::Agent),
        links: Vec::new(),
        body: String::new(),
    }
}

/// Superseding never raises a note whose score is negative: halving it
/// would. The scores are pinned where that went wrong.
#[hegel::test(test_cases = 200)]
#[hegel::explicit_test_case(scores = vec![-1.0_f32, -1.5], pick = 1_usize)]
fn superseding_never_raises_a_note_whatever_its_score(tc: TestCase) {
    let scores: Vec<f32> = tc.draw(
        gs::vecs(gs::floats::<f32>().min_value(-5.0).max_value(5.0))
            .min_size(1)
            .max_size(5),
    );
    let pick = tc.draw(gs::integers::<usize>().max_value(scores.len() - 1));
    let dir = tempfile::tempdir().unwrap();
    let mut notes = Notes::open(dir.path()).unwrap();
    let ids: Vec<String> = (0..scores.len()).map(|n| format!("n{n}")).collect();
    for id in &ids {
        notes.create(plain(id, "a note")).unwrap();
    }
    let index = Scored(ids.iter().cloned().zip(scores).collect());
    supersede_and_check(&mut notes, &index, "q", &ids[pick]);
}

/// A word only one note holds finds that note first, with the line that
/// holds it as the snippet.
#[hegel::test(test_cases = 100)]
#[hegel::explicit_test_case(
    drawn = vec![plain("a", "retry the header"), plain("b", "a cache")],
    id = "a".to_owned()
)]
fn a_word_only_one_note_holds_finds_it_first(tc: TestCase) {
    let drawn: Vec<Note> =
        tc.draw(gs::vecs(pool_note()).min_size(1).max_size(8));
    let dir = tempfile::tempdir().unwrap();
    let mut notes = Notes::open(dir.path()).unwrap();
    let mut index = Bm25::new();
    for note in drawn {
        if notes.create(note.clone()).is_ok() {
            index.upsert(&note.id, &index_text(&note)).unwrap();
        }
    }
    let ids: Vec<String> = notes.iter().map(|note| note.id.clone()).collect();
    let id = tc.draw(gs::sampled_from(ids));
    let mut note = notes.get(&id).unwrap().clone();
    note.body.push_str("\nzanzibar_only_here");
    note.valid_to = None;
    notes.update(note.clone()).unwrap();
    index.upsert(&note.id, &index_text(&note)).unwrap();
    // The word alone: any other word could be one a drawn note holds.
    let hits = recall(&notes, &index, "zanzibar_only_here", 3).unwrap();
    assert_eq!(hits[0].id, note.id);
    assert!(hits[0].snippet.contains("zanzibar_only_here"));
}
