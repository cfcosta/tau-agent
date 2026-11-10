//! A project of tau's own (ADR 0027): made empty with its first files on
//! `main`, checked out, opened again as it is, and read folder by folder
//! at a commit.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_testing::block_on;
use tau_vcs_host::{Identity, ProjectRepo, Vcs};

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
    let checkout = project.main_dir().to_owned();
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

/// A folder put on trunk with no workspace (ADR 0053): its files replace
/// what trunk held there, in a commit on top of trunk with the
/// description given, and the rest of trunk stays; the same files again
/// commit nothing, and the main workspace still reads as it was.
#[test]
fn a_folder_put_on_trunk_replaces_what_was_there() {
    let home = tempfile::tempdir().unwrap();
    let project = ProjectRepo::open_or_init(
        home.path().join("plugins"),
        Identity::default(),
        &files(),
    )
    .unwrap();
    let before = project.trunk().unwrap();
    let given = vec![
        ("plugin.luau".to_owned(), b"return { v = 2 }\n".to_vec()),
        ("tests/plugin.luau".to_owned(), b"-- a case\n".to_vec()),
    ];
    let after = project
        .commit_folder_on_trunk("hello", &given, "install hello 0.2.0")
        .unwrap();
    assert_ne!(after, before);
    assert_eq!(project.trunk().unwrap(), after);
    assert_eq!(project.parent_of(&after).unwrap(), Some(before.clone()));
    assert_eq!(
        project.description_of(&after).unwrap().trim(),
        "install hello 0.2.0"
    );
    let held: Vec<(String, Vec<u8>)> = project.files_under(&after, "").unwrap();
    assert_eq!(
        held,
        [
            ("README.md".to_owned(), b"Plugins.\n".to_vec()),
            (
                "hello/plugin.luau".to_owned(),
                b"return { v = 2 }\n".to_vec()
            ),
            (
                "hello/tests/plugin.luau".to_owned(),
                b"-- a case\n".to_vec()
            ),
        ]
    );
    // The same again is no change.
    assert_eq!(
        project
            .commit_folder_on_trunk("hello", &given, "install hello 0.2.0")
            .unwrap(),
        after
    );
    // The main workspace is where it was, on the trunk it started from.
    let checkout = project.main_dir().to_owned();
    let vcs = block_on(Vcs::open(checkout, Identity::default())).unwrap();
    let copy = block_on(vcs.working_copy()).unwrap();
    assert_eq!(copy.head, before);
}
