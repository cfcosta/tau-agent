//! Runs the retrieval evaluation (`tau_memory::eval`) and prints its
//! report: BM25, ColBERT alone and the two fused, at every level of
//! near-duplicates.
//!
//! ```text
//! tau-memory-eval [--keywords] [--json PATH]
//! ```
//!
//! `--keywords` runs BM25 alone, without loading docbert's model.
//! `--json` also writes every row to `PATH`. Embeddings are cached in
//! `$XDG_CACHE_HOME/tau/memory-eval`, so a second run encodes nothing.

use std::path::PathBuf;

use anyhow::{Context as _, bail};
use tau_memory::{
    colbert::{Colbert, Shared},
    docbert::Docbert,
    eval::{self, Leg, Semantic},
    index::Bm25,
};

fn main() -> anyhow::Result<()> {
    let mut keywords = false;
    let mut json = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--keywords" => keywords = true,
            "--json" => {
                json = Some(PathBuf::from(
                    args.next().context("--json takes a path")?,
                ))
            }
            other => {
                bail!("unknown argument {other:?}; see the source's header")
            }
        }
    }

    let fixture = eval::fixture()?;
    let cache = cache_dir();
    let encoder = (!keywords).then(|| Shared::new(Docbert::new()));
    let mut legs = vec![Leg::new("bm25", || Box::new(Bm25::new()))];
    if let Some(encoder) = &encoder {
        let cache = &cache;
        legs.push(Leg::new("colbert", move || {
            Box::new(Semantic(Colbert::new(encoder.clone()).cached(cache)))
        }));
        legs.push(Leg::new("hybrid", move || {
            Box::new(Colbert::new(encoder.clone()).cached(cache))
        }));
    }

    let work = std::env::temp_dir()
        .join(format!("tau-memory-eval-{}", std::process::id()));
    let report = eval::run(&fixture, &legs, &work, |k| {
        eprintln!(
            "level k = {k}: {} facts, each with {k} near-duplicates",
            fixture.facts.len()
        )
    });
    let _ = std::fs::remove_dir_all(&work);
    let report = report?;

    print!("{}", eval::table(&report));
    if let Some(path) = json {
        std::fs::write(&path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}

/// `$XDG_CACHE_HOME/tau/memory-eval`, or `~/.cache/tau/memory-eval`.
fn cache_dir() -> PathBuf {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(std::env::temp_dir)
        .join("tau/memory-eval")
}
