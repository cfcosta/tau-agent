//! What tau-skills keeps of the app, as the host draws it (ADR 0050, gap
//! K): the commands it lists, the page `nav` opens, and the folders
//! `reveal` may show.

use serde_json::json;
use tau_luau_plugins_host::{agent::Active, files::Files, runtime::Context};
use tau_testing::block_on_io;

#[test]
fn the_host_draws_its_commands_page_and_folders() {
    let home = tempfile::tempdir().unwrap();
    let person = home.path().join("person");
    std::fs::create_dir_all(person.join("release-notes")).unwrap();
    std::fs::write(
        person.join("release-notes/SKILL.md"),
        "---\nname: release-notes\ndescription: Writes release notes\n---\n\nGroup them.\n",
    )
    .unwrap();
    std::fs::create_dir_all(person.join("broken")).unwrap();
    let loaded = super::loaded();
    let files = Files::new(&loaded.declaration.uses.files, None, None)
        .with_data(home.path().join("data"))
        .with_skills(Some(person.clone()));
    let active = Active {
        settings: loaded.declaration.default_settings(),
        loaded,
    };
    let context = Context {
        run: json!({ "kind": "app" }),
        now: json!({}),
        settings: json!({}),
        state: json!({}),
        secrets: None,
        store: None,
        job: None,
    };
    let view = block_on_io(tau_luau_plugins_host::registry::app_view(
        &active, files, &context,
    ));
    assert_eq!(view.error, None);
    let names: Vec<&str> = view
        .commands
        .iter()
        .map(|command| command.name.as_str())
        .collect();
    assert_eq!(names, ["release-notes"]);
    assert_eq!(view.value["nav"]["badge"], json!("1"));
    let page = serde_json::to_string(view.page.as_ref().unwrap()).unwrap();
    assert!(page.contains("\"tau.reveal\""), "{page}");
    assert!(page.contains("not offered to the model"), "{page}");
    assert!(
        view.roots.contains(&person.display().to_string()),
        "{:?}",
        view.roots
    );
}
