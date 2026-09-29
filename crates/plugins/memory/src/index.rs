//! What finds notes by their words. [`Index`] is the seam: docbert
//! (BM25 fused with ColBERT) behind it in the app, and [`Bm25`], a small
//! in-process keyword index, where docbert's model is not wanted, as in
//! tests.
//!
//! An index holds one text per note ([`index_text`]) and answers a query
//! with note ids, best first. It is derived: rebuilt from the notes at
//! any time.

use std::collections::{BTreeMap, HashMap};

use crate::note::Note;

/// Finds notes by what they say.
pub trait Index: Send {
    /// Adds a note's text, or replaces it.
    fn upsert(&mut self, id: &str, text: &str) -> anyhow::Result<()>;

    fn remove(&mut self, id: &str) -> anyhow::Result<()>;

    /// Up to `limit` note ids for `query`, best first, with a score that
    /// only orders hits within one answer.
    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(String, f32)>>;
}

/// What an index holds for a note: its title, description, tags and
/// body.
pub fn index_text(note: &Note) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        note.title,
        note.description,
        note.tags.join(" "),
        note.body
    )
}

/// The words BM25 counts: runs of letters, digits and `_`, lowercased.
/// Identifiers such as `retry_after` stay whole; `retry-after` is two
/// words, as it is in a query.
pub fn words(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
}

/// Okapi BM25 over whole notes, with the usual `k1 = 1.2`, `b = 0.75`.
#[derive(Debug, Default, Clone)]
pub struct Bm25 {
    /// Each note's word counts and length.
    docs: BTreeMap<String, (HashMap<String, u32>, u32)>,
    /// How many notes hold each word.
    df: HashMap<String, u32>,
    total_len: u64,
}

pub const K1: f32 = 1.2;
pub const B: f32 = 0.75;

impl Bm25 {
    pub fn new() -> Self {
        Self::default()
    }

    fn forget(&mut self, id: &str) {
        if let Some((counts, len)) = self.docs.remove(id) {
            self.total_len -= u64::from(len);
            for word in counts.keys() {
                if let Some(n) = self.df.get_mut(word) {
                    *n -= 1;
                    if *n == 0 {
                        self.df.remove(word);
                    }
                }
            }
        }
    }
}

impl Index for Bm25 {
    fn upsert(&mut self, id: &str, text: &str) -> anyhow::Result<()> {
        self.forget(id);
        let mut counts: HashMap<String, u32> = HashMap::new();
        let mut len = 0;
        for word in words(text) {
            *counts.entry(word).or_default() += 1;
            len += 1;
        }
        for word in counts.keys() {
            *self.df.entry(word.clone()).or_default() += 1;
        }
        self.total_len += u64::from(len);
        self.docs.insert(id.to_owned(), (counts, len));
        Ok(())
    }

    fn remove(&mut self, id: &str) -> anyhow::Result<()> {
        self.forget(id);
        Ok(())
    }

    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(String, f32)>> {
        let n = self.docs.len() as f32;
        if n == 0.0 {
            return Ok(Vec::new());
        }
        let avg = self.total_len as f32 / n;
        let mut terms: Vec<String> = words(query).collect();
        terms.sort();
        terms.dedup();
        let mut hits: Vec<(String, f32)> = self
            .docs
            .iter()
            .filter_map(|(id, (counts, len))| {
                let score: f32 = terms
                    .iter()
                    .filter_map(|term| {
                        let tf = *counts.get(term)? as f32;
                        let df = self.df[term] as f32;
                        let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
                        let norm =
                            K1 * (1.0 - B + B * *len as f32 / avg.max(1.0));
                        Some(idf * tf * (K1 + 1.0) / (tf + norm))
                    })
                    .sum();
                (score > 0.0).then(|| (id.clone(), score))
            })
            .collect();
        // Best first; ties by id, so an answer never depends on order.
        hits.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hits.truncate(limit);
        Ok(hits)
    }
}
