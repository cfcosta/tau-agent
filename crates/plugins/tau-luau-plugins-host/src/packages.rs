//! What `tau-plugin` does to packages (ADR 0053): which it means, what each
//! declares and reaches once loaded, and how its tests go. The binary
//! parses and prints; `plugin_test` and tau-built-in-tests run the same.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;
use tau_luau_plugins::{Declaration, TestResult, Uses, packages};
use tokio_util::sync::CancellationToken;

use crate::runtime::{Files, MANIFEST, PLUGIN, load};

/// A package and the folder it was read from.
pub struct Package {
    pub folder: PathBuf,
    pub files: Files,
}

/// The packages `folders` name, read. With none, the folder `here` when
/// it holds a `tau.toml`, else every plugin tau comes with, in order. It
/// blocks.
pub fn chosen(
    folders: &[PathBuf],
    here: &Path,
) -> Result<Vec<Package>, String> {
    let folders = match folders {
        [] if here.join(MANIFEST).is_file() => vec![here.to_owned()],
        [] => {
            let dir = packages::find()?;
            // Read as starting reads them, so a package missing from the
            // list, or one it does not list, is the error it is there.
            crate::built_in::read(&dir)?;
            crate::built_in::listed(&dir)?
                .iter()
                .map(|name| dir.join(name))
                .collect()
        }
        folders => folders.to_vec(),
    };
    folders
        .into_iter()
        .map(|folder| {
            let files = read(&folder)?;
            Ok(Package { folder, files })
        })
        .collect()
}

/// The package at `folder`. It blocks.
pub fn read(folder: &Path) -> Result<Files, String> {
    if !folder.join(MANIFEST).is_file() {
        return Err(format!(
            "{} is not a plugin's folder: it has no {MANIFEST}",
            folder.display()
        ));
    }
    Files::read(folder, PLUGIN)
        .map_err(|error| format!("{}: {error}", folder.display()))
}

/// What a package declares and reaches, as `tau-plugin check` says it.
#[derive(Debug, Clone, Serialize)]
pub struct Summary {
    pub name: String,
    pub version: String,
    pub digest: String,
    /// What it reaches, in the words the Allow uses.
    pub reaches: Vec<String>,
    pub declaration: Declaration,
}

impl Summary {
    /// Its hooks, by the names a plugin gives them.
    pub fn hooks(&self) -> Vec<String> {
        let hooks =
            serde_json::to_value(self.declaration.hooks).unwrap_or_default();
        hooks
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, on)| on.as_bool() == Some(true))
            .map(|(name, _)| name.clone())
            .collect()
    }
}

/// Loads `files` as starting does, and says what it declares.
pub async fn summary(files: Files) -> Result<Summary, String> {
    let name = files.manifest.name.clone();
    let version = files.manifest.version.clone();
    let loaded = load(&name, files)
        .await
        .map_err(|error| format!("{name} does not load: {error}"))?;
    Ok(Summary {
        reaches: loaded.declaration.uses.grown_from(&Uses::default(), &name),
        name,
        version,
        digest: loaded.digest,
        declaration: loaded.declaration,
    })
}

/// What `test` does with a package's `fixtures/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fixtures {
    /// Holds what the tests make to the files there.
    Check,
    /// Writes what the tests make, and removes what none makes.
    Bless,
}

/// Loads `files`, the package at `folder`, and runs its tests; then holds
/// the fixtures they made, and its declaration, to `fixtures/`, or with
/// [`Fixtures::Bless`] writes them there.
pub async fn test(
    files: Files,
    folder: &Path,
    fixtures: Fixtures,
    cancel: CancellationToken,
) -> Result<Vec<TestResult>, String> {
    let name = files.manifest.name.clone();
    let loaded = load(&name, files)
        .await
        .map_err(|error| format!("{name} does not load: {error}"))?;
    let tested = loaded.tested(cancel).await;
    let mut results = tested.results;
    let mut made: Vec<(String, Value)> = tested
        .fixtures
        .into_iter()
        .map(|made| (made.name, made.value))
        .collect();
    let dir = folder.join("fixtures");
    let held = tokio::task::spawn_blocking(move || {
        if made.is_empty() && !dir.is_dir() {
            return Vec::new();
        }
        let declared =
            serde_json::to_value(&loaded.declaration).unwrap_or_default();
        made.push((DECLARATION.to_owned(), declared));
        hold(&dir, &made, fixtures)
    })
    .await
    .map_err(|error| error.to_string())?;
    results.extend(held);
    Ok(results)
}

/// The fixture every package with fixtures has: its declaration.
pub const DECLARATION: &str = "declaration";

/// Holds `made` to the files in `dir`, or writes them: a result for each,
/// and one for each file there that nothing made. It blocks.
fn hold(
    dir: &Path,
    made: &[(String, Value)],
    fixtures: Fixtures,
) -> Vec<TestResult> {
    let result = |name: &str, error: Option<String>| TestResult {
        file: "fixtures".to_owned(),
        name: name.to_owned(),
        passed: error.is_none(),
        error,
    };
    let mut results = Vec::new();
    for (name, value) in made {
        let path = dir.join(format!("{name}.json"));
        let error = match fixtures {
            Fixtures::Bless => write(&path, value).err(),
            Fixtures::Check => match std::fs::read_to_string(&path) {
                Err(_) => Some(format!(
                    "fixtures/{name}.json is missing: `tau-plugin test --bless` writes it"
                )),
                Ok(text) => match serde_json::from_str::<Value>(&text) {
                    Err(error) => Some(format!(
                        "fixtures/{name}.json is not JSON: {error}"
                    )),
                    Ok(kept) => difference("", &kept, value).map(|differs| {
                        format!(
                            "fixtures/{name}.json differs from what the tests \
                             make, {differs}; `tau-plugin test --bless` writes \
                             it anew"
                        )
                    }),
                },
            },
        };
        results.push(result(name, error));
    }
    for stale in unmade(dir, made) {
        let error = match fixtures {
            Fixtures::Bless => {
                std::fs::remove_file(dir.join(&stale)).err().map(|error| {
                    format!("cannot remove fixtures/{stale}: {error}")
                })
            }
            Fixtures::Check => Some(format!(
                "fixtures/{stale} is made by no `t.fixture`: \
                 `tau-plugin test --bless` removes it"
            )),
        };
        if let Some(error) = error {
            results.push(result(&stale, Some(error)));
        }
    }
    results
}

/// The `.json` files in `dir` that are none of `made`.
fn unmade(dir: &Path, made: &[(String, Value)]) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut unmade: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|file| {
            file.strip_suffix(".json")
                .is_some_and(|name| made.iter().all(|(made, _)| made != name))
        })
        .collect();
    unmade.sort();
    unmade
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    let shown = |error: &dyn std::fmt::Display| {
        format!("cannot write {}: {error}", path.display())
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|error| shown(&error))?;
    }
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| shown(&error))?
        + "\n";
    std::fs::write(path, text).map_err(|error| shown(&error))
}

/// Where `kept` and `made` first differ, under `at`, and what each holds
/// there; none when they are the same.
pub fn difference(at: &str, kept: &Value, made: &Value) -> Option<String> {
    let here = || if at.is_empty() { "/" } else { at };
    match (kept, made) {
        (Value::Object(kept), Value::Object(made)) => {
            let mut keys: Vec<&String> =
                kept.keys().chain(made.keys()).collect();
            keys.sort();
            keys.dedup();
            keys.into_iter().find_map(|key| {
                let at = format!("{at}/{key}");
                match (kept.get(key), made.get(key)) {
                    (Some(kept), Some(made)) => difference(&at, kept, made),
                    (Some(kept), None) => Some(format!(
                        "at {at}: it holds {kept}, the tests make nothing"
                    )),
                    (None, Some(made)) => Some(format!(
                        "at {at}: it holds nothing, the tests make {made}"
                    )),
                    (None, None) => None,
                }
            })
        }
        (Value::Array(kept), Value::Array(made)) => {
            let found = kept.iter().zip(made).enumerate().find_map(
                |(index, (kept, made))| {
                    difference(&format!("{at}/{index}"), kept, made)
                },
            );
            found.or_else(|| {
                (kept.len() != made.len()).then(|| {
                    format!(
                        "at {}: it holds {} items, the tests make {}",
                        here(),
                        kept.len(),
                        made.len()
                    )
                })
            })
        }
        _ if kept == made => None,
        _ => Some(format!(
            "at {}: it holds {kept}, the tests make {made}",
            here()
        )),
    }
}

/// tau's data directory, as the app finds it (`HostConfig::data_dir`):
/// `$XDG_DATA_HOME/tau`, or `~/.local/share/tau`.
pub fn data_dir() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local/share"))
        })
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tau")
}

/// What `install` did: the package, and trunk's commit that holds it.
#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    pub name: String,
    pub version: String,
    pub commit: String,
    pub repository: PathBuf,
}

/// `tau-plugin install` (ADR 0053): tests the package at `folder`, then
/// commits it to the plugins repository under `data` (made as the app
/// makes it, if it is not there), replacing a folder of the same name.
/// The plugin is then a new version like any other: active once the
/// person allows what it reaches.
pub async fn install(folder: &Path, data: &Path) -> Result<Installed, String> {
    let read = folder.to_owned();
    let files = tokio::task::spawn_blocking(move || self::read(&read))
        .await
        .map_err(|error| error.to_string())??;
    let name = files.manifest.name.clone();
    let version = files.manifest.version.clone();
    if crate::built_in::listed(&packages::find()?)
        .is_ok_and(|names| names.contains(&name))
    {
        return Err(format!(
            "{name} is one of tau's own: name yours otherwise"
        ));
    }
    let results =
        test(files, folder, Fixtures::Check, CancellationToken::new()).await?;
    let failed: Vec<String> = results
        .iter()
        .filter(|result| !result.passed)
        .map(|result| {
            format!(
                "{} › {}: {}",
                result.file,
                result.name,
                result.error.as_deref().unwrap_or("")
            )
        })
        .collect();
    if !failed.is_empty() {
        return Err(format!(
            "{name} is not installed: its tests fail\n  {}",
            failed.join("\n  ")
        ));
    }
    let repository = data.join(tau_luau_plugins::repository::ROOT);
    let (at, from) = (repository.clone(), folder.to_owned());
    let description = format!("install {name} {version}");
    let folder_name = name.clone();
    let commit = tokio::task::spawn_blocking(move || {
        let project = tau_vcs_host::ProjectRepo::open_or_init(
            &at,
            tau_vcs_host::Identity::default(),
            &tau_luau_plugins::repository::first_files(),
        )
        .map_err(|error| format!("{}: {error}", at.display()))?;
        let files = package_files(&from)?;
        project
            .commit_folder_on_trunk(&folder_name, &files, &description)
            .map_err(|error| format!("{}: {error}", at.display()))
    })
    .await
    .map_err(|error| error.to_string())??;
    Ok(Installed {
        name,
        version,
        commit,
        repository,
    })
}

/// A package's files, by their paths in it: `tau.toml`, the README, and
/// everything under `src/`, `tests/` and `fixtures/`. It blocks.
fn package_files(folder: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut found = Vec::new();
    for top in [MANIFEST, "README.md"] {
        if let Ok(bytes) = std::fs::read(folder.join(top)) {
            found.push((top.to_owned(), bytes));
        }
    }
    let mut dirs: Vec<String> =
        ["src", "tests", "fixtures"].map(str::to_owned).to_vec();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(folder.join(&dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(str::to_owned)
            else {
                continue;
            };
            let path = format!("{dir}/{name}");
            if entry.path().is_dir() {
                dirs.push(path);
            } else {
                let bytes = std::fs::read(entry.path())
                    .map_err(|error| format!("{path}: {error}"))?;
                found.push((path, bytes));
            }
        }
    }
    found.sort();
    Ok(found)
}
