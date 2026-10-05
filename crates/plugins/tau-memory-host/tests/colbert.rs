//! Late interaction: MaxSim against its definition, the index against a
//! model of live notes, the cache that spares encoding, and the
//! paraphrase BM25 alone would miss.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use hegel::{TestCase, generators as gs};
use tau_memory_host::{
    colbert::{
        Colbert,
        Encoder,
        EncoderError,
        Shared,
        Tokens,
        max_sim,
        read_tokens,
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
    fn documents(
        &mut self,
        texts: &[String],
    ) -> Result<Vec<Tokens>, EncoderError> {
        *self.encoded.lock().unwrap() += texts.len();
        Ok(texts.iter().map(|text| tokens(text)).collect())
    }

    fn query(&mut self, text: &str) -> Result<Tokens, EncoderError> {
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

/// Words from the vocabulary, none at all included.
#[hegel::composite]
fn text(tc: &TestCase) -> String {
    tc.draw(gs::vecs(gs::sampled_from(VOCAB.to_vec())).max_size(8))
        .join(" ")
}

/// MaxSim by its definition: nothing across dimensions; otherwise each
/// query token's best dot product with a document token, summed over
/// the tokens that have one (none do against an empty document).
fn reference(query: &Tokens, doc: &Tokens) -> f32 {
    if query.dim != doc.dim {
        return 0.0;
    }
    let mut sum = 0.0;
    for q in 0..query.count() {
        let mut best: Option<f32> = None;
        for d in 0..doc.count() {
            let dot: f32 = query
                .row(q)
                .iter()
                .zip(doc.row(d))
                .map(|(a, b)| a * b)
                .sum();
            best = Some(best.map_or(dot, |best| best.max(dot)));
        }
        sum += best.unwrap_or(0.0);
    }
    sum
}

#[hegel::test(test_cases = 200)]
fn max_sim_is_its_definition_and_grows_with_the_document(tc: TestCase) {
    let query = tokens(&tc.draw(text()));
    let doc_text = tc.draw(text());
    let mut doc = tokens(&doc_text);
    // The same values read with another width: a different model's.
    if tc.draw(gs::weighted_booleans(0.2)) {
        tc.event("dimensions differ");
        doc.dim = DIM / 2;
        assert_eq!(max_sim(&query, &doc), 0.0);
        return;
    }
    if doc.count() == 0 {
        tc.event("empty document");
    }
    assert!((max_sim(&query, &doc) - reference(&query, &doc)).abs() < 1e-4);
    // More tokens never lower it, from a document with any: each
    // maximum is over a superset.
    if doc.count() > 0 {
        let longer = tokens(&format!("{doc_text} {}", tc.draw(text())));
        assert!(max_sim(&query, &longer) >= max_sim(&query, &doc) - 1e-5);
    }
}

/// The index answers with every live note, scored by MaxSim against
/// the query, best first and ties by id, up to its limit.
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
    let query = tc.draw(text());
    let limit = tc.draw(gs::integers::<usize>().max_value(5));
    let hits = index.search(&query, limit).unwrap();
    let mut want: Vec<(String, f32)> = if query.trim().is_empty() {
        Vec::new()
    } else {
        live.iter()
            .map(|(id, text)| {
                (id.clone(), reference(&tokens(&query), &tokens(text)))
            })
            .collect()
    };
    want.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    want.truncate(limit);
    assert_eq!(hits, want, "{live:?}");
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

/// Two indexes over one shared encoder answer as two with their own.
#[hegel::test(test_cases = 50)]
fn a_shared_encoder_answers_as_its_own(tc: TestCase) {
    let fake = Fake::default();
    let shared = Shared::new(fake.clone());
    let mut a = Colbert::new(shared.clone());
    let mut b = Colbert::new(shared);
    let mut own = Colbert::new(Fake::default());
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(6));
    for n in 0..count {
        let text = tc.draw(text());
        let index = if n % 2 == 0 { &mut a } else { &mut b };
        index.upsert(&format!("n{n}"), &text).unwrap();
        own.upsert(&format!("n{n}"), &text).unwrap();
    }
    let query = tc.draw(text());
    let mut both = a.semantic(&query, 10).unwrap();
    both.extend(b.semantic(&query, 10).unwrap());
    both.sort_by(|x, y| y.1.total_cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    assert_eq!(both, own.semantic(&query, 10).unwrap());
    // Both indexes encoded through the one encoder.
    assert_eq!(*fake.encoded.lock().unwrap(), count);
}

#[hegel::test(test_cases = 100)]
fn an_embedding_file_reads_back(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let dim = tc.draw(gs::integers::<usize>().min_value(1).max_value(16));
    let count = tc.draw(gs::integers::<usize>().max_value(20));
    // Any value, NaN and infinities too: the file holds bits.
    let values: Vec<f32> = (0..dim * count)
        .map(|_| tc.draw(gs::floats::<f32>().allow_nan(true)))
        .collect();
    let tokens = Tokens { dim, values };
    let path = dir.path().join("x.emb");
    write_tokens(&path, &tokens).unwrap();
    let read = read_tokens(&path).unwrap();
    let bits = |tokens: &Tokens| -> Vec<u32> {
        tokens.values.iter().map(|value| value.to_bits()).collect()
    };
    assert_eq!((read.dim, bits(&read)), (tokens.dim, bits(&tokens)));
}

/// docbert's model itself: unit-length token vectors, and a paraphrase
/// ranked above an unrelated note. Downloads the model when it is not
/// cached, so it runs only when asked:
/// `cargo test -p tau-memory --features docbert -- --ignored`.
#[cfg(feature = "docbert")]
#[test]
#[ignore = "loads docbert's model"]
fn docbert_encodes_unit_tokens_and_finds_a_paraphrase() {
    use tau_memory_host::docbert::Docbert;

    let mut encoder = Docbert::new();
    let texts = vec![
        "the build is slow when the cache is cold".to_owned(),
        String::new(),
    ];
    for doc in encoder.documents(&texts).unwrap() {
        for row in 0..doc.count() {
            let norm: f32 =
                doc.row(row).iter().map(|x| x * x).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "{norm}");
        }
    }
    let mut index = Colbert::new(encoder);
    index
        .upsert(
            "slow-build",
            "compiling takes ages until the cache warms up",
        )
        .unwrap();
    index
        .upsert("lanes", "each session lane drains its queue in order")
        .unwrap();
    let hits = index.semantic("why is the build slow", 2).unwrap();
    assert_eq!(hits[0].0, "slow-build", "{hits:?}");
}
