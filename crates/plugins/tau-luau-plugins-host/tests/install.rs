//! `tau-plugin install` (ADR 0053): a package that passes its tests is
//! committed to the plugins repository, made as the app makes it, and the
//! registry reads it from trunk as any other version; one that fails is
//! not, and trunk stays.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use tau_luau_plugins::Standing;
use tau_luau_plugins_host::{packages, registry::Registry, scaffold};
use tau_testing::block_on_io;
use tau_vcs_host::{Identity, Project};

#[test]
fn an_installed_package_is_a_version_on_trunk() {
    let home = tempfile::tempdir().unwrap();
    let data = home.path().join("data");
    let folder = scaffold::write(home.path(), "greeter").unwrap();
    let installed = block_on_io(packages::install(&folder, &data)).unwrap();
    assert_eq!(installed.name, "greeter");
    let root = data.join(tau_luau_plugins::repository::ROOT);
    assert_eq!(installed.repository, root);

    let project =
        block_on_io(Project::open(root.clone(), Identity::default())).unwrap();
    let trunk = project.blocking().trunk().unwrap();
    assert_eq!(trunk, installed.commit);
    // The repository's first files, then the package.
    let files: Vec<String> = project
        .blocking()
        .files_under(&trunk, "")
        .unwrap()
        .into_iter()
        .map(|(path, _)| path)
        .collect();
    assert!(files.contains(&"AGENTS.md".to_owned()), "{files:?}");
    assert!(
        files.contains(&"greeter/src/plugin.luau".to_owned()),
        "{files:?}"
    );
    assert!(
        files.contains(&"greeter/tests/plugin.luau".to_owned()),
        "{files:?}"
    );

    // It reaches nothing past its state, so it is active by itself.
    let registry = Registry::at(
        root,
        home.path().join("allowed.json"),
        Identity::default(),
        || {},
    );
    block_on_io(registry.reload(&project, &trunk));
    let overview = block_on_io(registry.overview());
    let greeter = overview
        .plugins
        .iter()
        .find(|plugin| plugin.name == "greeter")
        .expect("listed");
    assert_eq!(greeter.standing, Standing::Active);

    // A version whose tests fail is not installed, and trunk stays.
    let test = folder.join("tests/plugin.luau");
    let text = std::fs::read_to_string(&test).unwrap();
    std::fs::write(&test, text.replace("\"Hello, Ada!\"", "\"Hi, Ada!\""))
        .unwrap();
    let error = block_on_io(packages::install(&folder, &data)).unwrap_err();
    assert!(error.contains("its tests fail"), "{error}");
    assert_eq!(project.blocking().trunk().unwrap(), trunk);
}
