//! Late interaction and fusion: MaxSim and RRF against their
//! definitions, the index against a model of live notes, the cache that
//! spares encoding, and the paraphrase BM25 alone would miss.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use hegel::{TestCase, generators as gs};
use tau_memory::{
    colbert::{
        Colbert,
        Encoder,
        RRF_K,
        Tokens,
        max_sim,
        read_tokens,
        rrf,
        write_tokens,
    },
    index::{Index, words},
};

const DIM: usize = 8;

/// A word's vector: fixed, unit length. Synonyms share one, as a model
/// places them close.
fn vector(word: &str) -> Vec<f32> {
    let word = match word {
        "automobile" => "car",
        "slow" => "sluggish",
        other => other,
    };
    let mut seed = word
        .bytes()
        .fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(u64::from(b)));
    let raw: Vec<f32> = (0..DIM)
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

/// Encodes each word as its vector, and counts document encodes.
#[derive(Clone, Default)]
struct Fake {
    encoded: Arc<Mutex<usize>>,
}

fn tokens(text: &str) -> Tokens {
    let values: Vec<f32> = words(text).flat_map(|word| vector(&word)).collect();
    Tokens { dim: DIM, values }
}

impl Encoder for Fake {
    fn documents(&mut self, texts: &[String]) -> anyhow::Result<Vec<Tokens>> {
        *self.encoded.lock().unwrap() += texts.len();
        Ok(texts.iter().map(|text| tokens(text)).collect())
    }

    fn query(&mut self, text: &str) -> anyhow::Result<Tokens> {
        Ok(tokens(text))
    }

    fn model(&self) -> &str {
        "fake"
    }
}

const VOCAB: [&str; 10] = [
    "car",
    "automobile",
    "slow",
    "sluggish",
    "lane",
    "drain",
    "retry",
    "cache",
    "fork",
    "header",
];

#[hegel::composite]
fn text(tc: TestCase) -> String {
    tc.draw(
        gs::vecs(gs::sampled_from(VOCAB.to_vec()))
            .min_size(1)
            .max_size(8),
    )
    .join(" ")
}

#[hegel::test(test_cases = 200)]
fn max_sim_is_its_definition_and_grows_with_the_document(tc: TestCase) {
    let query = tokens(&tc.draw(text()));
    let doc_text = tc.draw(text());
    let doc = tokens(&doc_text);
    // Each query token's best match, summed.
    let want: f32 = (0..query.count())
        .map(|q| {
            (0..doc.count())
                .map(|d| {
                    query
                        .row(q)
                        .iter()
                        .zip(doc.row(d))
                        .map(|(a, b)| a * b)
                        .sum::<f32>()
                })
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .sum();
    assert!((max_sim(&query, &doc) - want).abs() < 1e-4);
    // More tokens never lower it: each maximum is over a superset.
    let longer = tokens(&format!("{doc_text} {}", tc.draw(text())));
    assert!(max_sim(&query, &longer) >= max_sim(&query, &doc) - 1e-5);
}

#[hegel::test(test_cases = 200)]
fn rrf_is_its_formula(tc: TestCase) {
    let ids = vec!["a", "b", "c", "d", "e"];
    let lists: Vec<Vec<String>> = (0..tc
        .draw(gs::integers::<usize>().min_value(1).max_value(3)))
        .map(|_| {
            // A ranking: distinct ids in drawn order.
            let mut pool = ids.clone();
            let mut list = Vec::new();
            for _ in 0..tc.draw(gs::integers::<usize>().max_value(5)) {
                let at =
                    tc.draw(gs::integers::<usize>().max_value(pool.len() - 1));
                list.push(pool.remove(at).to_owned());
            }
            list
        })
        .collect();
    let fused = rrf(&lists);
    let mut want: BTreeMap<String, f32> = BTreeMap::new();
    for list in &lists {
        for (rank, id) in list.iter().enumerate() {
            *want.entry(id.clone()).or_default() +=
                1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    assert_eq!(fused.len(), want.len());
    for (id, score) in &fused {
        assert!((score - want[id]).abs() < 1e-6);
    }
    assert!(fused.windows(2).all(|pair| pair[0].1 >= pair[1].1));
}

#[hegel::test(test_cases = 100)]
fn the_index_answers_only_with_live_notes(tc: TestCase) {
    let mut index = Colbert::new(Fake::default());
    let mut live: BTreeMap<String, String> = BTreeMap::new();
    for _ in 0..tc.draw(gs::integers::<usize>().max_value(12)) {
        let id = tc
            .draw(gs::sampled_from(vec!["a", "b", "c", "d"]))
            .to_owned();
        if tc.draw(gs::booleans()) {
            let text = tc.draw(text());
            index.upsert(&id, &text).unwrap();
            live.insert(id, text);
        } else {
            index.remove(&id).unwrap();
            live.remove(&id);
        }
    }
    let hits = index.search(&tc.draw(text()), 10).unwrap();
    assert!(
        hits.iter().all(|(id, _)| live.contains_key(id)),
        "{hits:?} vs {live:?}"
    );
    assert!(hits.len() <= live.len());
}

#[test]
fn a_paraphrase_finds_what_its_words_do_not() {
    let mut index = Colbert::new(Fake::default());
    index
        .upsert("slow-car", "the car is sluggish on cold starts")
        .unwrap();
    index.upsert("lanes", "lanes drain and retry").unwrap();
    // No word in common with the note; its synonyms are.
    let hits = index.search("automobile slow", 5).unwrap();
    assert_eq!(hits[0].0, "slow-car");
}

#[hegel::test(test_cases = 50)]
fn the_cache_spares_encoding_what_did_not_change(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let notes: BTreeMap<String, String> = (0..tc
        .draw(gs::integers::<usize>().min_value(1).max_value(5)))
        .map(|n| (format!("n{n}"), tc.draw(text())))
        .collect();
    let fake = Fake::default();
    let open = |fake: &Fake| {
        let mut index = Colbert::new(fake.clone()).cached(dir.path());
        for (id, text) in &notes {
            index.upsert(id, text).unwrap();
        }
        index
    };
    let first = open(&fake);
    assert_eq!(*fake.encoded.lock().unwrap(), notes.len());
    // Reopened: nothing encoded again, and the same answers.
    let again = Fake::default();
    let second = open(&again);
    assert_eq!(*again.encoded.lock().unwrap(), 0);
    let query = tc.draw(text());
    assert_eq!(
        first.search(&query, 5).unwrap(),
        second.search(&query, 5).unwrap()
    );
    // One note changed: only it is encoded, and its old cache is gone.
    let changed = Fake::default();
    let mut third = Colbert::new(changed.clone()).cached(dir.path());
    for (id, text) in &notes {
        let text = if id == "n0" {
            format!("{text} fork")
        } else {
            text.clone()
        };
        third.upsert(id, &text).unwrap();
    }
    assert_eq!(*changed.encoded.lock().unwrap(), 1);
    let files = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(files, notes.len());
    // Removed: its cache goes with it, and no other note's.
    third.remove("n0").unwrap();
    let left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left.len(), notes.len() - 1);
    assert!(left.iter().all(|name| !name.starts_with("n0.")));
}

#[hegel::test(test_cases = 100)]
fn an_embedding_file_reads_back(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let dim = tc.draw(gs::integers::<usize>().min_value(1).max_value(16));
    let count = tc.draw(gs::integers::<usize>().max_value(20));
    let values: Vec<f32> = (0..dim * count)
        .map(|_| {
            tc.draw(gs::floats::<f32>().allow_nan(false).allow_infinity(false))
        })
        .collect();
    let tokens = Tokens { dim, values };
    let path = dir.path().join("x.emb");
    write_tokens(&path, &tokens).unwrap();
    assert_eq!(read_tokens(&path).unwrap(), tokens);
}
