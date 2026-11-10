//! `tau-plugin`, a command line for Luau plugins (ADR 0053): it loads,
//! checks and tests packages with no window and no Rust.
//!
//! - `tau-plugin new <name> [--in <dir>]`: writes a package that loads
//!   and passes its tests; `--built-in` writes one of tau's own into the
//!   checkout's `plugins/` and lists it in `built-in.toml`.
//! - `tau-plugin install <folder>`: tests the package, then commits it to
//!   the person's plugins repository; it is active once they allow what
//!   it reaches on the Plugins page.
//! - `tau-plugin check [<folder>...]`: loads each as starting does and
//!   says what it declares and what it reaches.
//! - `tau-plugin test [<folder>...]`: runs each package's tests; a line per
//!   package and one per failure, or one per test with `--verbose`.
//!   `--filter <text>` keeps the tests with `text` in their file or name;
//!   `--bless` writes the fixtures the tests make into `fixtures/`, and
//!   removes those none makes; `--watch` runs them again each time a
//!   package changes.
//!
//! With no folder, either takes the folder it is run in when that holds a
//! `tau.toml`, else every plugin tau comes with. `--json` prints what it
//! found as one JSON value. It exits 0 when all is well, 1 when a test
//! fails or a package does not load, and 2 when it was asked wrongly.

#![allow(
    clippy::disallowed_methods,
    reason = "a command line is a synchronous entry point (ADR 0028)"
)]

use std::{path::PathBuf, process::ExitCode};

use serde_json::json;
use tau_luau_plugins::TestResult;
use tau_luau_plugins_host::{
    built_in,
    packages::{self, Fixtures, Package},
};
use tokio_util::sync::CancellationToken;

const USAGE: &str = "\
usage: tau-plugin <command> [<folder>...] [options]

commands:
  new      write a package that passes its tests: tau-plugin new <name>
  install  test a package, then commit it to your plugins repository
  check    load each package and say what it declares and reaches
  test     run each package's tests

options:
  --json            print what was found as one JSON value
  --filter <text>   test: only the tests with <text> in their file or name
  --verbose         test: a line per test, not only per failure
  --bless           test: write the fixtures the tests make
  --watch           test: run again each time a package changes
  --in <dir>        new: the folder to write it in, else the one it is run in
  --built-in        new: one of tau's own, in the checkout's plugins/

With no folder: the folder it is run in when it holds a tau.toml, else
every plugin tau comes with.";

/// What was asked.
struct Asked {
    command: String,
    folders: Vec<PathBuf>,
    json: bool,
    verbose: bool,
    bless: bool,
    watch: bool,
    built_in: bool,
    within: Option<PathBuf>,
    filter: Option<String>,
}

fn parse(args: &[String]) -> Result<Asked, String> {
    let mut args = args.iter();
    let command = args.next().ok_or("no command given")?.clone();
    let mut asked = Asked {
        command,
        folders: Vec::new(),
        json: false,
        verbose: false,
        bless: false,
        watch: false,
        built_in: false,
        within: None,
        filter: None,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => asked.json = true,
            "--verbose" | "-v" => asked.verbose = true,
            "--bless" => asked.bless = true,
            "--watch" => asked.watch = true,
            "--built-in" => asked.built_in = true,
            "--in" => {
                asked.within = Some(PathBuf::from(
                    args.next().ok_or("--in takes a folder")?,
                ));
            }
            "--filter" => {
                asked.filter =
                    Some(args.next().ok_or("--filter takes a text")?.clone());
            }
            flag if flag.starts_with('-') => {
                return Err(format!("{flag} is not an option"));
            }
            folder => asked.folders.push(PathBuf::from(folder)),
        }
    }
    Ok(asked)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(args.first().map(String::as_str), Some("-h" | "--help")) {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let asked = match parse(&args) {
        Ok(asked) => asked,
        Err(error) => return asked_wrongly(&error),
    };
    if !matches!(asked.command.as_str(), "new" | "install" | "check" | "test") {
        return asked_wrongly(&format!("{} is not a command", asked.command));
    }
    let here = std::env::current_dir().unwrap_or_default();
    let runtime = match built_in::runtime() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("tau-plugin: {error}");
            return ExitCode::FAILURE;
        }
    };
    let all = match asked.command.as_str() {
        "install" => match asked.folders.as_slice() {
            [folder] => runtime.block_on(install(&asked, folder)),
            _ => return asked_wrongly("install takes the package's folder"),
        },
        "new" => match new(&asked, &here) {
            Ok(folder) => match packages::chosen(&[folder], &here) {
                Ok(made) => runtime.block_on(test(&asked, made)),
                Err(error) => fail(&error),
            },
            Err(Wrong::Asked(error)) => return asked_wrongly(&error),
            Err(Wrong::Failed(error)) => fail(&error),
        },
        command => loop {
            let packages = match packages::chosen(&asked.folders, &here) {
                Ok(packages) => packages,
                // Watching, a package half-saved is waited out.
                Err(error) if asked.watch && command == "test" => {
                    fail(&error);
                    wait_for_change(&[], &asked.folders, &here);
                    continue;
                }
                Err(error) => break fail(&error),
            };
            let digests = digests(&packages);
            let all = runtime.block_on(async {
                if command == "check" {
                    check(&asked, packages).await
                } else {
                    test(&asked, packages).await
                }
            });
            if !(asked.watch && command == "test") {
                break all;
            }
            wait_for_change(&digests, &asked.folders, &here);
        },
    };
    if all {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `install`: whether the package passed and is committed.
async fn install(asked: &Asked, folder: &std::path::Path) -> bool {
    match packages::install(folder, &packages::data_dir()).await {
        Ok(installed) => {
            if asked.json {
                println!("{}", json!(installed));
            } else {
                println!(
                    "installed {} {} in {} at {}: it is active once you \
                     allow what it reaches on the Plugins page",
                    installed.name,
                    installed.version,
                    installed.repository.display(),
                    &installed.commit[..installed.commit.len().min(12)]
                );
            }
            true
        }
        Err(error) => fail(&error),
    }
}

fn fail(error: &str) -> bool {
    eprintln!("tau-plugin: {error}");
    false
}

/// How `new` went wrong: asked wrongly, or failed doing it.
enum Wrong {
    Asked(String),
    Failed(String),
}

/// `new`: writes the package the arguments name, and says where.
fn new(asked: &Asked, here: &std::path::Path) -> Result<PathBuf, Wrong> {
    use tau_luau_plugins_host::scaffold;
    let [name] = asked.folders.as_slice() else {
        return Err(Wrong::Asked("new takes the package's name".into()));
    };
    let name = name.to_string_lossy();
    scaffold::check_name(&name, asked.built_in).map_err(Wrong::Asked)?;
    let dir = if asked.built_in {
        scaffold::checkout_plugins(here).map_err(Wrong::Failed)?
    } else {
        asked.within.clone().unwrap_or_else(|| here.to_owned())
    };
    let folder = scaffold::write(&dir, &name).map_err(Wrong::Failed)?;
    if asked.built_in {
        scaffold::list(&dir, &name).map_err(Wrong::Failed)?;
    }
    println!("wrote {}", folder.display());
    Ok(folder)
}

/// Each package's digest, which any change to its files changes.
fn digests(packages: &[Package]) -> Vec<String> {
    packages
        .iter()
        .map(|package| package.files.digest())
        .collect()
}

/// Waits until the packages' digests are no longer `digests`, checking
/// twice a second.
fn wait_for_change(
    digests: &[String],
    folders: &[PathBuf],
    here: &std::path::Path,
) {
    println!("-- waiting for a change (Ctrl-C stops)");
    while digests == chosen_digests(folders, here).as_slice() {
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// [`digests`] of the packages as they are now; none when they cannot be
/// read, which is a change too.
fn chosen_digests(folders: &[PathBuf], here: &std::path::Path) -> Vec<String> {
    packages::chosen(folders, here)
        .map(|packages| digests(&packages))
        .unwrap_or_default()
}

fn asked_wrongly(error: &str) -> ExitCode {
    eprintln!("tau-plugin: {error}\n\n{USAGE}");
    ExitCode::from(2)
}

/// `check`: whether every package loads.
async fn check(asked: &Asked, packages: Vec<Package>) -> bool {
    let mut all = true;
    let mut found = Vec::new();
    for Package { files, .. } in packages {
        let name = files.manifest.name.clone();
        match packages::summary(files).await {
            Ok(summary) => {
                if !asked.json {
                    print_summary(&summary);
                }
                found.push(json!(summary));
            }
            Err(error) => {
                all = false;
                if !asked.json {
                    eprintln!("{error}");
                }
                found.push(json!({ "name": name, "error": error }));
            }
        }
    }
    if asked.json {
        println!("{}", json!(found));
    }
    all
}

fn print_summary(summary: &packages::Summary) {
    let declaration = &summary.declaration;
    println!(
        "{} {} ({})",
        summary.name,
        summary.version,
        &summary.digest[..summary.digest.len().min(12)]
    );
    let tools: Vec<&str> = declaration
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect();
    let commands: Vec<&str> = declaration
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    let jobs: Vec<&str> = declaration
        .jobs
        .iter()
        .map(|job| job.name.as_str())
        .collect();
    for (label, items) in [
        ("tools", tools),
        (
            "hooks",
            summary.hooks().iter().map(String::as_str).collect(),
        ),
        (
            "actions",
            declaration.actions.iter().map(String::as_str).collect(),
        ),
        ("commands", commands),
        ("jobs", jobs),
        (
            "pages",
            declaration.pages.iter().map(String::as_str).collect(),
        ),
        (
            "fills",
            declaration.fills.iter().map(String::as_str).collect(),
        ),
    ] {
        if !items.is_empty() {
            println!("  {label}: {}", items.join(", "));
        }
    }
    if summary.reaches.is_empty() {
        println!("  reaches nothing past its own state");
    } else {
        println!("  reaches: {}", summary.reaches.join("; "));
    }
}

/// `test`: whether every package loads and passes.
async fn test(asked: &Asked, packages: Vec<Package>) -> bool {
    let mut all = true;
    let mut found = Vec::new();
    let (mut passed, mut ran) = (0, 0);
    let many = packages.len() > 1;
    let fixtures = if asked.bless {
        Fixtures::Bless
    } else {
        Fixtures::Check
    };
    for Package { folder, files } in packages {
        let name = files.manifest.name.clone();
        let cancel = CancellationToken::new();
        match packages::test(files, &folder, fixtures, cancel).await {
            Ok(results) => {
                let results: Vec<TestResult> = results
                    .into_iter()
                    .filter(|test| kept(asked, test))
                    .collect();
                let ok = results.iter().filter(|test| test.passed).count();
                passed += ok;
                ran += results.len();
                all &= ok == results.len();
                if !asked.json {
                    print_results(asked, &name, &results);
                }
                found.push(json!({ "name": name, "tests": results }));
            }
            Err(error) => {
                all = false;
                if !asked.json {
                    eprintln!("{error}");
                }
                found.push(json!({ "name": name, "error": error }));
            }
        }
    }
    if asked.json {
        println!("{}", json!(found));
    } else if many {
        println!("{passed} of {ran} tests pass");
    }
    all
}

/// Whether `--filter` keeps `test`.
fn kept(asked: &Asked, test: &TestResult) -> bool {
    asked.filter.as_deref().is_none_or(|filter| {
        test.file.contains(filter) || test.name.contains(filter)
    })
}

fn print_results(asked: &Asked, name: &str, results: &[TestResult]) {
    let passed = results.iter().filter(|test| test.passed).count();
    if results.is_empty() {
        println!("{name}: no tests");
        return;
    }
    println!("{name}: {passed} of {} tests pass", results.len());
    for test in results {
        if test.passed {
            if asked.verbose {
                println!("  ok    {} › {}", test.file, test.name);
            }
        } else {
            println!(
                "  FAIL  {} › {}: {}",
                test.file,
                test.name,
                test.error.as_deref().unwrap_or("")
            );
        }
    }
}
