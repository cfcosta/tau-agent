//! Late-interaction search over notes: each note's text as ColBERT token
//! vectors, scored against a query's by MaxSim.
//!
//! ColBERT alone, not fused with BM25: in the retrieval evaluation
//! ([`crate::eval`]) it found verbatim queries as well as BM25 and the
//! fusion did, and paraphrases far more often than either, while the
//! fusion let BM25 rank near-duplicates that share a query's words above
//! the answer.
//!
//! The model is behind [`Encoder`]: docbert's in the app (the `docbert`
//! feature), a deterministic stand-in in tests. Embeddings are cached on
//! disk by the text they encode, so reopening a scope encodes only what
//! changed. Every note is scored against the query: a scope is small
//! enough that this is fast, and it is exact.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::index::{Index, IndexError};

/// One text as token vectors, each L2-normalized: `tokens × dim` values,
/// row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct Tokens {
    pub dim: usize,
    pub values: Vec<f32>,
}

impl Tokens {
    pub fn count(&self) -> usize {
        self.values.len().checked_div(self.dim).unwrap_or(0)
    }

    pub fn row(&self, i: usize) -> &[f32] {
        &self.values[i * self.dim..(i + 1) * self.dim]
    }
}

/// Why an encoder could not embed text: its model would not load, or a
/// tensor would not read back. Only docbert's model fails; without it
/// there is no way to build one.
#[derive(Debug, thiserror::Error)]
pub enum EncoderError {
    #[cfg(feature = "docbert")]
    #[error(transparent)]
    Model(#[from] docbert_pylate::ColbertError),
    #[cfg(feature = "docbert")]
    #[error(transparent)]
    Tensor(#[from] candle_core::Error),
}

/// Turns text into ColBERT token vectors.
pub trait Encoder: Send {
    fn documents(
        &mut self,
        texts: &[String],
    ) -> Result<Vec<Tokens>, EncoderError>;
    fn query(&mut self, text: &str) -> Result<Tokens, EncoderError>;
    /// Names the model, so a cache from another model is not reused.
    fn model(&self) -> &str;
}

/// One encoder shared by several indexes, so a model is loaded once for
/// every scope.
pub struct Shared<E> {
    inner: Arc<Mutex<E>>,
    model: String,
}

impl<E: Encoder> Shared<E> {
    pub fn new(encoder: E) -> Self {
        Self {
            model: encoder.model().to_owned(),
            inner: Arc::new(Mutex::new(encoder)),
        }
    }
}

impl<E> Clone for Shared<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            model: self.model.clone(),
        }
    }
}

impl<E: Encoder> Encoder for Shared<E> {
    fn documents(
        &mut self,
        texts: &[String],
    ) -> Result<Vec<Tokens>, EncoderError> {
        self.inner.lock().expect("not poisoned").documents(texts)
    }

    fn query(&mut self, text: &str) -> Result<Tokens, EncoderError> {
        self.inner.lock().expect("not poisoned").query(text)
    }

    fn model(&self) -> &str {
        &self.model
    }
}

/// ColBERT's MaxSim: for each query token, its best dot product with any
/// of the document's tokens, summed.
pub fn max_sim(query: &Tokens, doc: &Tokens) -> f32 {
    if query.dim != doc.dim {
        return 0.0;
    }
    (0..query.count())
        .map(|q| {
            let row = query.row(q);
            (0..doc.count())
                .map(|d| {
                    row.iter().zip(doc.row(d)).map(|(a, b)| a * b).sum::<f32>()
                })
                .fold(f32::NEG_INFINITY, f32::max)
        })
        .filter(|best| best.is_finite())
        .sum()
}

/// Late interaction over a scope's notes.
pub struct Colbert<E: Encoder> {
    /// Behind a lock: encoding needs the model mutably, and search does
    /// not otherwise change the index.
    encoder: Mutex<E>,
    model: String,
    embeddings: BTreeMap<String, Tokens>,
    /// Where embeddings are cached, when anywhere.
    cache: Option<PathBuf>,
}

impl<E: Encoder> Colbert<E> {
    pub fn new(encoder: E) -> Self {
        Self {
            model: encoder.model().to_owned(),
            encoder: Mutex::new(encoder),
            embeddings: BTreeMap::new(),
            cache: None,
        }
    }

    /// Caches embeddings under `dir`, one file per note, named by the
    /// model and a hash of the text: an unchanged note is not encoded
    /// again.
    pub fn cached(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache = Some(dir.into());
        self
    }

    /// Runs `f` on the encoder.
    pub fn with_encoder<T>(&self, f: impl FnOnce(&mut E) -> T) -> T {
        f(&mut self.encoder.lock().expect("not poisoned"))
    }

    fn cache_path(&self, id: &str, text: &str) -> Option<PathBuf> {
        let dir = self.cache.as_ref()?;
        let key = fnv1a(&[self.model.as_bytes(), &[0], text.as_bytes()]);
        Some(dir.join(format!("{id}.{key:016x}.emb")))
    }

    fn embed(&mut self, id: &str, text: &str) -> Result<Tokens, IndexError> {
        let path = self.cache_path(id, text);
        if let Some(path) = &path
            && let Ok(tokens) = read_tokens(path)
        {
            return Ok(tokens);
        }
        let tokens = self
            .with_encoder(|encoder| encoder.documents(&[text.to_owned()]))?
            .pop()
            .ok_or(IndexError::NoTokens)?;
        if let Some(path) = path {
            // Older versions of this note's cache go; a failed write only
            // costs an encode next time.
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
                forget_cached(dir, id);
            }
            let _ = write_tokens(&path, &tokens);
        }
        Ok(tokens)
    }
}

impl<E: Encoder> Index for Colbert<E> {
    fn upsert(&mut self, id: &str, text: &str) -> Result<(), IndexError> {
        let tokens = self.embed(id, text)?;
        self.embeddings.insert(id.to_owned(), tokens);
        Ok(())
    }

    fn remove(&mut self, id: &str) -> Result<(), IndexError> {
        self.embeddings.remove(id);
        if let Some(dir) = &self.cache {
            forget_cached(dir, id);
        }
        Ok(())
    }

    fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, IndexError> {
        self.semantic(query, limit)
    }
}

impl<E: Encoder> Colbert<E> {
    /// Up to `limit` notes by MaxSim against `query`, best first.
    pub fn semantic(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(String, f32)>, IndexError> {
        if self.embeddings.is_empty() || query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let tokens = self.with_encoder(|encoder| encoder.query(query))?;
        let mut scored: Vec<(String, f32)> = self
            .embeddings
            .iter()
            .map(|(id, doc)| (id.clone(), max_sim(&tokens, doc)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        scored.truncate(limit);
        Ok(scored)
    }
}

/// FNV-1a over `parts`: stable across Rust versions, unlike the
/// standard hasher, so a cache survives a toolchain upgrade.
fn fnv1a(parts: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Removes every cached embedding of note `id`.
fn forget_cached(dir: &Path, id: &str) {
    let prefix = format!("{id}.");
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // `<id>.<16 hex>.emb`: the id is everything before the hash.
        if name
            .strip_prefix(&prefix)
            .is_some_and(|rest| rest.len() == 20 && rest.ends_with(".emb"))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// An embedding file: `dim` and the value count as little-endian u32,
/// then the values as little-endian f32.
pub fn write_tokens(path: &Path, tokens: &Tokens) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(8 + tokens.values.len() * 4);
    bytes.extend_from_slice(&(tokens.dim as u32).to_le_bytes());
    bytes.extend_from_slice(&(tokens.values.len() as u32).to_le_bytes());
    for value in &tokens.values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    let tmp = path.with_extension("tmp");
    fs::File::create(&tmp)?.write_all(&bytes)?;
    fs::rename(tmp, path)
}

pub fn read_tokens(path: &Path) -> io::Result<Tokens> {
    let mut bytes = Vec::new();
    fs::File::open(path)?.read_to_end(&mut bytes)?;
    let bad = || {
        io::Error::new(io::ErrorKind::InvalidData, "a broken embedding file")
    };
    let word = |at: usize| -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            bytes.get(at..at + 4).ok_or_else(bad)?.try_into().unwrap(),
        ))
    };
    let dim = word(0)? as usize;
    let count = word(4)? as usize;
    if bytes.len() != 8 + count * 4
        || (dim == 0 && count != 0)
        || !count.is_multiple_of(dim.max(1))
    {
        return Err(bad());
    }
    let values = bytes[8..]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect();
    Ok(Tokens { dim, values })
}
