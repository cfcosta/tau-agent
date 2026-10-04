//! Skills in real runs: a scripted model, the store, and a skills
//! folder. The run's instructions list the skills; the tool gives the
//! model a skill's instructions and folder, and says which skills there
//! are when asked for one that is not.

use std::{fs, path::Path};

use futures_util::StreamExt;
use serde_json::json;
use tau_agent::{agent::Agent, event::RunEvent};
use tau_skills::{Loaded, SKILL_FILE, host::SkillsPlugin, scan};
use tau_store::Store;
use tau_testing::{block_on_io, scripted::ScriptedModel};

fn skill(dir: &Path, name: &str, description: &str, body: &str) {
    let folder = dir.join(name);
    fs::create_dir_all(folder.join("scripts")).unwrap();
    fs::write(
        folder.join(SKILL_FILE),
        format!(
            "---\nname: {name}\ndescription: {description}\n---\n\n{body}\n"
        ),
    )
    .unwrap();
    fs::write(folder.join("scripts/run.sh"), "echo hi\n").unwrap();
}

/// Runs `input` to the end, returning its events.
fn run(agent: &Agent, input: &str) -> Vec<RunEvent> {
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        let mut run = agent.start(input, &store);
        let events: Vec<RunEvent> = run.events().collect().await;
        run.outcome().await.unwrap();
        events
    })
}

/// The `skill` calls' results: their text, details, and whether they
/// failed.
fn results(events: &[RunEvent]) -> Vec<(String, Option<Loaded>, bool)> {
    events
        .iter()
        .filter_map(|event| match event {
            RunEvent::ToolEnd {
                output, is_error, ..
            } => Some((
                output.text_content(),
                output
                    .details
                    .clone()
                    .map(|details| serde_json::from_value(details).unwrap()),
                *is_error,
            )),
            _ => None,
        })
        .collect()
}

#[test]
fn the_model_loads_a_listed_skill() {
    let home = tempfile::tempdir().unwrap();
    skill(
        home.path(),
        "release-notes",
        "Writes release notes between two tags",
        "Group the commits by kind.",
    );
    skill(
        home.path(),
        "code-review",
        "Reviews the diff",
        "Look for bugs.",
    );
    let llm = ScriptedModel::new()
        .turn(|t| t.tool_call("skill", json!({ "name": "release-notes" })))
        .turn(|t| t.text("Grouped."));
    let agent = Agent::new(llm.clone())
        .plugin(SkillsPlugin::new(scan::scan(home.path())));
    let events = run(&agent, "write the notes");

    // The instructions list both, and keep the skills' own text out.
    let instructions = llm.requests()[0].settings.instructions.clone().unwrap();
    assert!(instructions.contains(scan::HEADING));
    assert!(instructions.contains("- code-review: Reviews the diff"));
    assert!(
        instructions
            .contains("- release-notes: Writes release notes between two tags")
    );
    assert!(!instructions.contains("Group the commits"));

    // The call gives its instructions, without the frontmatter, and its
    // folder, for the files it names.
    let [(text, Some(loaded), false)] = &results(&events)[..] else {
        panic!("{:?}", results(&events));
    };
    assert!(text.contains("Group the commits by kind."));
    assert!(!text.contains("description:"));
    let folder = home.path().join("release-notes");
    assert!(text.contains(&folder.display().to_string()));
    assert_eq!(loaded.name, "release-notes");
    assert_eq!(loaded.file, folder.join(SKILL_FILE));
}

#[test]
fn an_unknown_skill_names_the_ones_there_are() {
    let home = tempfile::tempdir().unwrap();
    skill(
        home.path(),
        "code-review",
        "Reviews the diff",
        "Look for bugs.",
    );
    let llm = ScriptedModel::new()
        // A name like a path does not reach outside the folder.
        .turn(|t| t.tool_call("skill", json!({ "name": "../code-review" })))
        .turn(|t| t.text("Sorry."));
    let agent = Agent::new(llm.clone())
        .plugin(SkillsPlugin::new(scan::scan(home.path())));
    let events = run(&agent, "review");
    let [(text, None, true)] = &results(&events)[..] else {
        panic!("{:?}", results(&events));
    };
    assert!(text.contains("no skill \"../code-review\""), "{text}");
    assert!(text.contains("code-review"), "{text}");
}
