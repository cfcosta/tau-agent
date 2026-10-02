//! Offline evaluation CLI. Live mode fails closed until a guarded transport exists.

use std::{env, fs, path::PathBuf};

use tau_codemode_eval::runner::{
    LiveLimits,
    evaluate_offline_with_timings,
    reject_live,
};

fn parse_positive<T: std::str::FromStr>(
    value: Option<String>,
    name: &str,
) -> Result<T, String> {
    value
        .ok_or_else(|| format!("missing value for {name}"))?
        .parse()
        .map_err(|_| format!("invalid value for {name}"))
}

fn parse_arguments() -> Result<(Option<PathBuf>, bool, bool, LiveLimits), String>
{
    let mut args = env::args().skip(1);
    let mut output = None;
    let mut live = false;
    let mut timings = false;
    let mut limits = LiveLimits::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--output" => output = Some(PathBuf::from(args.next().ok_or("missing value for --output")?)),
            "--live" => live = true,
            "--timings" => timings = true,
            "--max-provider-attempts" => limits.max_provider_attempts = Some(parse_positive(args.next(), &arg)?),
            "--max-usd" => limits.max_usd = Some(parse_positive(args.next(), &arg)?),
            "--max-seconds" => limits.max_seconds = Some(parse_positive(args.next(), &arg)?),
            "--max-output-tokens" => limits.max_output_tokens = Some(parse_positive(args.next(), &arg)?),
            "--help" => return Err("usage: tau-codemode-eval [--output PATH] [--timings] [--live --max-provider-attempts N --max-usd USD --max-seconds N --max-output-tokens N]".into()),
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    Ok((output, live, timings, limits))
}

fn main() -> Result<(), String> {
    let (output, live, timings, limits) = parse_arguments()?;
    if live {
        return reject_live(limits);
    }
    if limits != LiveLimits::default() {
        return Err("provider limits require --live".into());
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    let report = runtime.block_on(evaluate_offline_with_timings(timings))?;
    let json = serde_json::to_string_pretty(&report)
        .map_err(|error| error.to_string())?;
    if let Some(path) = output {
        fs::write(path, format!("{json}\n"))
            .map_err(|error| error.to_string())?;
    } else {
        println!("{json}");
    }
    Ok(())
}
