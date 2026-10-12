//! Runs the recursive-language-model evaluation against a real model and
//! prints a table per task, size and arm. It costs money.

use std::{path::PathBuf, process::ExitCode, sync::Arc};

use tau_ai::{
    chatgpt::{AccountId, ChatGpt, Store},
    client::OpenAi,
    llm::Llm,
};
use tau_rlm_eval::{
    access,
    arm::Arm,
    metrics,
    runner::{self, Config},
    task::Kind,
};

const USAGE: &str = "\
tau-rlm-eval: long-context tasks (Recursive Language Models, arXiv
2512.24601) under four arms: plain (coding tools), codemode (tools.infer at
its default limits), codemode_wide (limits raised) and rlm (limits raised and
the paper's strategy in the instructions). It makes real model calls.

Usage: tau-rlm-eval [options]

  --task NAME       count, top, needle, docs (repeat, or comma-separated)
  --arm NAME        plain, codemode, codemode_wide, rlm
  --lines N         input sizes in lines (repeat, or comma-separated;
                    default 1000,6000)
  --trials N        repetitions (default 1)
  --model ID        the model (default: gpt-5.5)
  --budget-usd USD  stop once this much is spent
  --json PATH       also write every trial and summary to PATH
  --chatgpt ID      use this ChatGPT account's plan
  --max-turns N     turns a run may take (default 40)
  --docs DIR        the documents for the docs task (default: docs)
  --work DIR        where inputs are made (default: the temp dir)
  --help            print this
";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tau-rlm-eval: {error}");
            ExitCode::FAILURE
        }
    }
}

#[allow(
    clippy::disallowed_methods,
    reason = "the process's entry point runs its async work here (ADR 0028)"
)]
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut kinds, mut arms, mut sizes) = (Vec::new(), Vec::new(), Vec::new());
    let (mut trials, mut max_turns) = (1u32, 40u32);
    let (mut model, mut budget, mut json, mut chatgpt, mut work) =
        (None, None, None::<PathBuf>, None, None::<PathBuf>);
    let mut docs = None::<PathBuf>;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} takes a value"));
        match arg.as_str() {
            "--task" => {
                for name in value()?.split(',') {
                    kinds.push(
                        Kind::parse(name.trim())
                            .ok_or(format!("no task {name:?}"))?,
                    );
                }
            }
            "--arm" => {
                for name in value()?.split(',') {
                    arms.push(
                        Arm::parse(name.trim())
                            .ok_or(format!("no arm {name:?}"))?,
                    );
                }
            }
            "--lines" => {
                for n in value()?.split(',') {
                    sizes.push(
                        n.trim().parse::<usize>().map_err(|e| e.to_string())?,
                    );
                }
            }
            "--trials" => {
                trials = value()?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?
            }
            "--max-turns" => {
                max_turns = value()?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?
            }
            "--model" => model = Some(value()?),
            "--budget-usd" => {
                budget =
                    Some(value()?.parse::<f64>().map_err(|e| e.to_string())?)
            }
            "--json" => json = Some(value()?.into()),
            "--chatgpt" => {
                let id = value()?;
                chatgpt = Some(
                    AccountId::parse(&id)
                        .ok_or(format!("{id:?} is not an account id"))?,
                );
            }
            "--work" => work = Some(value()?.into()),
            "--docs" => docs = Some(value()?.into()),
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => {
                return Err(format!("unknown argument {other:?}; see --help"));
            }
        }
    }
    let access = access::resolve(chatgpt, access::config_dir().as_deref())
        .ok_or("no model access: sign in with ChatGPT in tau, or pass --chatgpt ID")?;
    let work = work.unwrap_or_else(std::env::temp_dir);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let root = tempfile::Builder::new()
        .prefix("tau-rlm-eval-")
        .tempdir_in(&work)
        .map_err(|e| e.to_string())?;
    let mut config = Config::new(
        model.unwrap_or_else(|| access::DEFAULT_MODEL.into()),
        root.path(),
    );
    if !kinds.is_empty() {
        config.kinds = kinds;
    }
    if !arms.is_empty() {
        config.arms = arms;
    }
    if !sizes.is_empty() {
        config.sizes = sizes;
    }
    if let Some(docs) = docs {
        config.docs = docs;
    }
    config.trials = trials;
    config.max_turns = max_turns;
    config.budget_usd = budget;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let llm: Arc<dyn Llm> = {
        let _guard = runtime.enter();
        Arc::new(OpenAi::chatgpt(
            ChatGpt::new(
                Store::open(&access.store).map_err(|e| e.to_string())?,
            ),
            access.account.clone(),
        ))
    };
    eprintln!(
        "{} trials on {} through {}; budget {}",
        config.planned(),
        config.model,
        access.label(),
        budget.map_or_else(|| "none".to_owned(), |usd| format!("${usd:.2}")),
    );
    let mut progress = |t: &metrics::Trial| {
        eprintln!(
            "{} {} {} #{}: {} (answer {:?}, expected {}; {} turns, {} infer, ${:.4}, {:.0}s, {})",
            t.kind.name(),
            t.lines,
            t.arm.name(),
            t.trial,
            if t.score.exact { "ok" } else { "wrong" },
            t.score.answer,
            t.expected,
            t.run.turns,
            t.run.infer_calls,
            t.run.cost_usd,
            t.run.wall_ms as f64 / 1000.0,
            t.run.stop,
        );
    };
    let report = runtime
        .block_on(runner::evaluate(&config, llm, &mut progress))
        .map_err(|e| e.to_string())?;
    print!("{}", metrics::table(&report.summaries));
    println!("spent ${:.4}", report.spent_usd);
    if let Some(why) = &report.stopped {
        println!("stopped early: {why}");
    }
    if let Some(path) = &json {
        std::fs::write(
            path,
            serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
