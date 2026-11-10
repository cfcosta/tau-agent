//! tau-skills on real folders (ADR 0050, gap K): what Luau's fake files
//! cannot say, the frontmatter read by the host's YAML parser and the
//! skills tau ships in the package.

mod app;
mod scan;
mod shipped;

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use tau_luau_plugins_host::{
    files::Files,
    runtime::{Context, Hook, Loaded, NoReach, WithFiles, load},
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

/// A skill, as the plugin's `app` keeps it.
#[derive(Debug, Clone, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub files: usize,
    pub ignored: Vec<String>,
    pub builtin: bool,
}

/// A folder that is not a skill, and why.
#[derive(Debug, Clone, Deserialize)]
pub struct Problem {
    pub dir: PathBuf,
    #[allow(dead_code, reason = "read when a test fails")]
    pub reason: String,
}

/// What the folders hold, and the list a run's instructions get of them.
#[derive(Debug, Clone, Deserialize)]
pub struct Skills {
    pub found: Vec<Skill>,
    pub problems: Vec<Problem>,
    #[serde(default)]
    pub section: Option<String>,
}

pub fn loaded() -> Loaded {
    let files =
        tau_luau_plugins_host::built_in::files(tau_luau_plugins::names::SKILLS)
            .unwrap();
    block_on_io(load(tau_luau_plugins::names::SKILLS, files)).unwrap()
}

/// What the plugin reads of the person's skills in `person` and those
/// shipped in `package/skills`, and the instructions' list.
pub fn read(person: Option<&Path>, package: Option<&Path>) -> Skills {
    let loaded = loaded();
    let home = tempfile::tempdir().unwrap();
    let files = Files::new(&loaded.declaration.uses.files, None, None)
        .with_package(package.map(Path::to_owned))
        .with_data(home.path().join("data"))
        .with_skills(person.map(Path::to_owned));
    let reach = std::sync::Arc::new(WithFiles {
        reach: std::sync::Arc::new(NoReach),
        files: std::sync::Arc::new(files),
    });
    let context = Context {
        run: json!({ "kind": "main", "call": "c1" }),
        now: json!({}),
        settings: json!({}),
        state: json!({}),
        secrets: None,
        store: None,
        job: None,
    };
    let call = |hook: Hook| {
        block_on_io(loaded.call(
            &hook,
            json!({}),
            &context,
            reach.clone(),
            CancellationToken::new(),
        ))
    };
    let app = call(Hook::App);
    assert_eq!(app.error, None);
    let mut skills: Skills =
        serde_json::from_value(app.value["skills"].clone()).unwrap();
    let started = call(Hook::Start);
    assert_eq!(started.error, None);
    skills.section = match &started.value["instructions"] {
        Value::String(text) => Some(text.clone()),
        _ => None,
    };
    skills
}

/// The skills in `dir`, the person's, with none shipped.
pub fn scan(dir: &Path) -> Skills {
    read(Some(dir), None)
}
