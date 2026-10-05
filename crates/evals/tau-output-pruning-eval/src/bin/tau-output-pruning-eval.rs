//! Runs the output pruning evaluation against the real Jev and prints a
//! table per workload. Jev costs about $0.042 per million input tokens:
//! a full run costs cents.
//!
//! See [`USAGE`] for the flags.

use std::{
    num::{ParseFloatError, ParseIntError},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
};

use tau_jev::{Jev, MissingApiKey, TypeSafe};
use tau_output_pruning_eval::{
    metrics::{self, Trial},
    runner::{self, Config, EvalError},
    workload::Kind,
};

/// Why the evaluation did not run, or stopped.
#[derive(Debug, thiserror::Error)]
enum CliError {
    #[error("{0} takes a value; see --help")]
    NoValue(String),
    #[error("--seeds takes a count: {0}")]
    Seeds(#[from] ParseIntError),
    #[error("--seeds must be at least 1")]
    NoSeeds,
    #[error("--budget-usd takes dollars: {0}")]
    Budget(#[from] ParseFloatError),
    #[error("--budget-usd must be more than zero")]
    NoBudget,
    #[error("unknown argument {0:?}; see --help")]
    Unknown(String),
    #[error("no workload is called {0:?}; see --list")]
    NoWorkload(String),
    #[error(
        "TYPESAFE_API_KEY is not set: the evaluation asks the real Jev. Set \
         it to a TypeSafe key and run again. Nothing was run."
    )]
    NoKey(#[from] MissingApiKey),
    #[error("cannot write {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Eval(#[from] EvalError),
}

const USAGE: &str = "\
tau-output-pruning-eval: how well fast compaction's output pruning keeps
the lines a task needs. Each workload is a conversation ending in a long
command output, generated from a seed, with needles (lines the task needs)
among noise. The real plugin prunes it through the agent loop, asking the
real Jev, which costs money (cents for a full run).

Usage: tau-output-pruning-eval [options]

  --workload NAME   only these workloads (repeat, or comma-separated)
  --seeds N         seeds 0 to N-1 of each workload (default 1)
  --budget-usd USD  stop once Jev spend passes this
  --json PATH       also write every trial and summary to PATH
  --whole           bash returns every output whole, as a shell without
                    limits would (default: tau's bash, which keeps the
                    last 2,000 lines or 50 KB and spills the rest)
  --work DIR        where archives and spilled outputs go (default: a new
                    directory in the temp dir, removed afterwards)
  --list            list the workloads, and exit
  --help            print this, and exit

Needs TYPESAFE_API_KEY.
";

struct Options {
    workloads: Vec<String>,
    seeds: u64,
    budget: Option<f64>,
    json: Option<PathBuf>,
    work: Option<PathBuf>,
    whole: bool,
    list: bool,
    help: bool,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, CliError> {
    let mut options = Options {
        workloads: Vec::new(),
        seeds: 1,
        budget: None,
        json: None,
        work: None,
        whole: false,
        list: false,
        help: false,
    };
    while let Some(arg) = args.next() {
        let mut value =
            || args.next().ok_or_else(|| CliError::NoValue(arg.clone()));
        match arg.as_str() {
            "--workload" => options
                .workloads
                .extend(value()?.split(',').map(|name| name.trim().to_owned())),
            "--seeds" => {
                options.seeds = value()?.parse()?;
                if options.seeds == 0 {
                    return Err(CliError::NoSeeds);
                }
            }
            "--budget-usd" => {
                let usd: f64 = value()?.parse()?;
                if usd.is_nan() || usd <= 0.0 {
                    return Err(CliError::NoBudget);
                }
                options.budget = Some(usd);
            }
            "--json" => options.json = Some(value()?.into()),
            "--work" => options.work = Some(value()?.into()),
            "--whole" => options.whole = true,
            "--list" => options.list = true,
            "--help" | "-h" => options.help = true,
            other => return Err(CliError::Unknown(other.to_owned())),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Every message carries its cause already.
            eprintln!("tau-output-pruning-eval: {error}");
            ExitCode::FAILURE
        }
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "the process's entry point runs its async work here (ADR 0028)"
)]
fn run() -> Result<(), CliError> {
    let options = parse(std::env::args().skip(1))?;
    if options.help {
        print!("{USAGE}");
        return Ok(());
    }
    if options.list {
        for kind in Kind::ALL {
            println!("{:<20} {}", kind.name(), kind.describe());
        }
        return Ok(());
    }
    let kinds = if options.workloads.is_empty() {
        Kind::ALL.to_vec()
    } else {
        options
            .workloads
            .iter()
            .map(|name| {
                Kind::parse(name)
                    .ok_or_else(|| CliError::NoWorkload(name.clone()))
            })
            .collect::<Result<_, _>>()?
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let jev: Arc<dyn Jev> = {
        // The client must be created inside the runtime.
        let _guard = runtime.enter();
        Arc::new(TypeSafe::from_env()?)
    };

    let scratch = match &options.work {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            None
        }
        None => Some(
            tempfile::Builder::new()
                .prefix("tau-output-pruning-eval-")
                .tempdir()?,
        ),
    };
    let work = options
        .work
        .clone()
        .or_else(|| scratch.as_ref().map(|dir| dir.path().to_owned()))
        .expect("one or the other");
    let mut config = Config::new(work);
    config.kinds = kinds;
    config.seeds = (0..options.seeds).collect();
    config.budget_usd = options.budget;
    if options.whole {
        config.bash = runner::Bash::Whole;
    }

    eprintln!(
        "{} trials against Jev; budget {}",
        config.kinds.len() * config.seeds.len(),
        options
            .budget
            .map_or_else(|| "none".to_owned(), |usd| format!("${usd:.2}")),
    );
    let mut progress = |trial: &Trial| {
        eprintln!(
            "{} #{}: {}/{} needles, {}/{} chunks kept (answers: {} noise, {} uncertain, {} needed), {} -> {} tokens{}, {} requests, ${:.4}, {:.1}s{}",
            trial.workload.name(),
            trial.seed,
            trial.retained,
            trial.needles,
            trial.kept_chunks.map_or("-".to_owned(), |n| n.to_string()),
            trial.chunks.map_or("-".to_owned(), |n| n.to_string()),
            trial.answers.noise,
            trial.answers.uncertain,
            trial.answers.needed,
            trial.tokens_before,
            trial.tokens_after,
            if trial.replaced {
                ""
            } else {
                " (not replaced)"
            },
            trial.requests,
            trial.cost_usd,
            trial.latency_ms as f64 / 1000.0,
            trial
                .error
                .as_ref()
                .map(|error| format!(", failed: {error}"))
                .unwrap_or_default(),
        );
        for missed in &trial.missed {
            eprintln!("  missed: {missed}");
        }
    };
    let report =
        runtime.block_on(runner::evaluate(&config, jev, &mut progress))?;

    print!("{}", metrics::table(&report.summaries));
    println!("spent ${:.4}", report.spent_usd);
    if let Some(why) = &report.stopped {
        println!("stopped early: {why}");
    }
    if let Some(path) = &options.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)?).map_err(
            |source| CliError::Write {
                path: path.clone(),
                source,
            },
        )?;
    }
    Ok(())
}
