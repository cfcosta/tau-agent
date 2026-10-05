//! A project of tau's own (ADR 0027): made empty with its first files on
//! `main`, checked out, opened again as it is, and read folder by folder
//! at a commit.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_testing::block_on;
use tau_vcs_host::{DEFAULT_WORKSPACE, Identity, ProjectRepo, Vcs};

fn files() -> Vec<(String, String)> {
    vec![
        ("README.md".into(), "Plugins.\n".into()),
        ("hello/plugin.luau".into(), "return {}\n".into()),
        ("hello/lib/greet.luau".into(), "return 1\n".into()),
    ]
}

#[test]
fn a_new_project_holds_its_first_files_on_main() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("plugins");
    let project =
        ProjectRepo::open_or_init(&root, Identity::default(), &files())
            .unwrap();
    assert_eq!(project.trunk_name().unwrap(), "main");
    let trunk = project.trunk().unwrap();
    // The checkout has them, and nothing is left to commit.
    let checkout = project.workspace_dir(DEFAULT_WORKSPACE);
    assert_eq!(
        std::fs::read_to_string(checkout.join("hello/lib/greet.luau")).unwrap(),
        "return 1\n"
    );
    let vcs = block_on(Vcs::open(checkout, Identity::default())).unwrap();
    let copy = block_on(vcs.working_copy()).unwrap();
    assert!(copy.is_committed(), "{copy:?}");
    assert_eq!(copy.head, trunk);

    // Read by folder at the commit.
    let hello: Vec<(String, String)> = project
        .files_under(&trunk, "hello")
        .unwrap()
        .into_iter()
        .map(|(path, bytes)| (path, String::from_utf8(bytes).unwrap()))
        .collect();
    assert_eq!(
        hello,
        [
            ("hello/lib/greet.luau".to_owned(), "return 1\n".to_owned()),
            ("hello/plugin.luau".to_owned(), "return {}\n".to_owned()),
        ]
    );
    assert_eq!(project.files_under(&trunk, "").unwrap().len(), 3);
    assert!(project.files_under(&trunk, "missing").unwrap().is_empty());
    assert!(project.files_under(&trunk, "README.md").unwrap().is_empty());

    // Opened again, it is the same project.
    let again =
        ProjectRepo::open_or_init(&root, Identity::default(), &[]).unwrap();
    assert_eq!(again.trunk().unwrap(), trunk);
}
