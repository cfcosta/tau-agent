//! Runs the end-to-end memory evaluation against a real model and prints
//! a table per arm. Every trial is two agent runs: this costs money.
//!
//! See [`USAGE`] for the flags.

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use anyhow::{Context as _, bail};
use tau_ai::{client::OpenAi, codex::CodexAuth, llm::Llm};
use tau_memory_e2e::{
    access::{self, Access},
    arm::{self, Arm},
    metrics::{self, Trial},
    runner::{self, Budget, Config},
    scenario::{self, Variant},
};

const USAGE: &str = "\
tau-memory-e2e: tau-memory's end-to-end evaluation. Each trial runs an agent
twice on a small generated repository: the first run finds a fact doing one
task, the second needs it for another, with the fact unchanged (stable) or
changed by a commit in between (changed). Arms: what the second run knows
of the first. It makes real model calls, which cost money.

Usage: tau-memory-e2e [options]

  --scenario NAME   only these scenarios (repeat, or comma-separated)
  --arm NAME        only these arms: none, memory_md, transcripts, memory,
                    memory_consolidate (repeat, or comma-separated)
  --variant NAME    stable or changed (default: both)
  --trials N        repetitions of each scenario, variant and arm (default 1)
  --model ID        the model (default: tau-ui's for the access: gpt-6-sol on
                    a ChatGPT sign-in, gpt-5.5 on an API key)
  --budget-usd USD  stop once this much is spent
  --json PATH       also write every trial and summary to PATH
  --codex PATH      reach the model with this ChatGPT sign-in file
  --max-turns N     turns a run may take (default 40)
  --work DIR        where trial repositories are made (default: the temp dir)
  --keywords        search memory and transcripts with BM25 even when built
                    with docbert
  --list            list the scenarios and arms, and exit
  --help            print this, and exit

Access, first found: --codex, OPENAI_API_KEY, tau's ChatGPT sign-in
($XDG_CONFIG_HOME/tau/codex.json), tau's saved API key
($XDG_CONFIG_HOME/tau/openai-key).
";

struct Options {
    scenarios: Vec<String>,
    arms: Vec<String>,
    variants: Vec<String>,
    trials: u32,
    model: Option<String>,
    budget: Option<f64>,
    json: Option<PathBuf>,
    codex: Option<PathBuf>,
    max_turns: u32,
    work: Option<PathBuf>,
    keywords: bool,
    list: bool,
    help: bool,
}

fn parse(mut args: impl Iterator<Item = String>) -> anyhow::Result<Options> {
    let mut options = Options {
        scenarios: Vec::new(),
        arms: Vec::new(),
        variants: Vec::new(),
        trials: 1,
        model: None,
        budget: None,
        json: None,
        codex: None,
        max_turns: arm::MAX_TURNS,
        work: None,
        keywords: false,
        list: false,
        help: false,
    };
    while let Some(arg) = args.next() {
        let mut value = || {
            args.next()
                .with_context(|| format!("{arg} takes a value; see --help"))
        };
        let split = |text: String| -> Vec<String> {
            text.split(',').map(|part| part.trim().to_owned()).collect()
        };
        match arg.as_str() {
            "--scenario" => options.scenarios.extend(split(value()?)),
            "--arm" => options.arms.extend(split(value()?)),
            "--variant" => options.variants.extend(split(value()?)),
            "--trials" => {
                options.trials =
                    value()?.parse().context("--trials takes a count")?
            }
            "--model" => options.model = Some(value()?),
            "--budget-usd" => {
                let usd: f64 =
                    value()?.parse().context("--budget-usd takes dollars")?;
                if usd.is_nan() || usd <= 0.0 {
                    bail!("--budget-usd must be more than zero");
                }
                options.budget = Some(usd);
            }
            "--json" => options.json = Some(value()?.into()),
            "--codex" => options.codex = Some(value()?.into()),
            "--max-turns" => {
                options.max_turns =
                    value()?.parse().context("--max-turns takes a count")?
            }
            "--work" => options.work = Some(value()?.into()),
            "--keywords" => options.keywords = true,
            "--list" => options.list = true,
            "--help" | "-h" => options.help = true,
            other => bail!("unknown argument {other:?}; see --help"),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tau-memory-e2e: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let options = parse(std::env::args().skip(1))?;
    if options.help {
        print!("{USAGE}");
        return Ok(());
    }
    if options.list {
        for scenario in scenario::SCENARIOS {
            println!("scenario {}: {}", scenario.name, scenario.fact);
        }
        for arm in Arm::ALL {
            println!("arm {}", arm.name());
        }
        return Ok(());
    }

    let mut config = Config::new(String::new(), PathBuf::new());
    if !options.scenarios.is_empty() {
        config.scenarios = options
            .scenarios
            .iter()
            .map(|name| {
                scenario::find(name).with_context(|| {
                    format!("no scenario is called {name:?}; see --list")
                })
            })
            .collect::<anyhow::Result<_>>()?;
    }
    if !options.arms.is_empty() {
        config.arms = options
            .arms
            .iter()
            .map(|name| {
                Arm::parse(name)
                    .with_context(|| format!("no arm is called {name:?}"))
            })
            .collect::<anyhow::Result<_>>()?;
    }
    if !options.variants.is_empty() {
        config.variants = options
            .variants
            .iter()
            .map(|name| {
                Variant::parse(name).with_context(|| {
                    format!("{name:?} is not a variant: stable or changed")
                })
            })
            .collect::<anyhow::Result<_>>()?;
    }
    config.trials = options.trials;
    config.max_turns = options.max_turns;
    config.budget = Budget::new(options.budget);

    let Some(access) = access::resolve(
        options.codex.clone(),
        std::env::var("OPENAI_API_KEY").ok(),
        access::config_dir().as_deref(),
    ) else {
        bail!(
            "no model access: set OPENAI_API_KEY, pass --codex PATH, or \
             sign in with tau first. Nothing was run."
        );
    };
    config.model = options
        .model
        .clone()
        .unwrap_or_else(|| access.default_model().to_owned());
    #[cfg(feature = "docbert")]
    if !options.keywords {
        config.index = arm::semantic_index();
        config.index_name = "colbert".into();
    }
    #[cfg(not(feature = "docbert"))]
    let _ = options.keywords;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let llm: Arc<dyn Llm> = {
        // Clients must be created inside the runtime.
        let _guard = runtime.enter();
        match &access {
            Access::Codex(path) => Arc::new(OpenAi::codex(
                CodexAuth::from_file(path).with_context(|| {
                    format!("cannot read the sign-in at {}", path.display())
                })?,
            )),
            Access::ApiKey(key) => Arc::new(OpenAi::new(key.clone())),
        }
    };

    let work = options.work.clone().unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&work)?;
    let root = tempfile::Builder::new()
        .prefix("tau-memory-e2e-")
        .tempdir_in(&work)?;
    config.work = root.path().to_owned();

    eprintln!(
        "{} trials ({} runs) on {} through {}, searching with {}; budget {}",
        config.planned(),
        config.planned() * 2,
        config.model,
        access.label(),
        config.index_name,
        options
            .budget
            .map_or_else(|| "none".to_owned(), |usd| format!("${usd:.2}")),
    );
    let mut progress = |trial: &Trial| {
        eprintln!(
            "{} {} {} #{}: {} ({} calls, ${:.4}, {:.1}s){}",
            trial.scenario,
            trial.variant.name(),
            trial.arm.name(),
            trial.trial,
            if trial.success() { "ok" } else { "failed" },
            trial.second.tool_calls,
            trial.cost_usd(),
            trial.second.wall_ms as f64 / 1000.0,
            if trial.stale_used == Some(true) {
                ", used the old fact"
            } else {
                ""
            },
        );
    };
    let report = runtime.block_on(runner::evaluate(
        &config,
        &|_| llm.clone(),
        &mut progress,
    ))?;

    print!("{}", metrics::table(&report.summaries));
    println!("spent ${:.4}", report.spent_usd);
    if let Some(why) = &report.stopped {
        println!("stopped early: {why}");
    }
    if let Some(path) = &options.json {
        std::fs::write(path, serde_json::to_string_pretty(&report)?)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    Ok(())
}
