//! Runs the retrieval evaluation (`tau_memory_host::eval`) and prints its
//! report: BM25, ColBERT (what memory searches with) and the two fused,
//! at every level of near-duplicates.
//!
//! ```text
//! tau-memory-eval [--keywords] [--json PATH]
//! ```
//!
//! `--keywords` runs BM25 alone, without loading docbert's model.
//! `--json` also writes every row to `PATH`. Embeddings are cached in
//! `$XDG_CACHE_HOME/tau/memory-eval`, so a second run encodes nothing.

use std::{path::PathBuf, process::ExitCode};

use tau_memory_host::{
    colbert::{Colbert, Shared},
    docbert::Docbert,
    eval::{self, EvalError, Hybrid, Leg},
    index::Bm25,
};

/// Why the evaluation did not run, or stopped.
#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("--json takes a path")]
    NoPath,
    #[error("unknown argument {0:?}; see the source's header")]
    Unknown(String),
    #[error(transparent)]
    Eval(#[from] EvalError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tau-memory-eval: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), CliError> {
    let mut keywords = false;
    let mut json = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--keywords" => keywords = true,
            "--json" => {
                json = Some(PathBuf::from(args.next().ok_or(CliError::NoPath)?))
            }
            other => return Err(CliError::Unknown(other.to_owned())),
        }
    }

    let fixture = eval::fixture()?;
    let cache = cache_dir();
    let encoder = (!keywords).then(|| Shared::new(Docbert::new()));
    let mut legs = vec![Leg::new("bm25", || Box::new(Bm25::new()))];
    if let Some(encoder) = &encoder {
        let cache = &cache;
        legs.push(Leg::new("colbert", move || {
            Box::new(Colbert::new(encoder.clone()).cached(cache))
        }));
        legs.push(Leg::new("hybrid", move || {
            Box::new(Hybrid::new(Colbert::new(encoder.clone()).cached(cache)))
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
            .map_err(|source| CliError::Write { path, source })?;
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
