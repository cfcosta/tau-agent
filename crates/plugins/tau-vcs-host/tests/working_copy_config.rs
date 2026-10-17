//! A conflict, a checkout and a diff follow the person's jj config as
//! the jj CLI does (`docs/reference/vcs.md`, "Identity and jj config"):
//!
//! - `ui.conflict-marker-style` is how a conflicted file is written to
//!   the working copy, and how `vcs_diff` and a project's diff show it;
//! - `working-copy.eol-conversion` is how a checkout writes line ends;
//! - `working-copy.exec-bit-change` is whether a snapshot sees a file's
//!   executable bit change.
//!
//! Each test makes up the person's config in a temporary directory, and
//! reads no real one.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::path::{Path, PathBuf};

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_agent::{plugin::Plugin as _, tool::ToolCtx};
use tau_testing::{block_on_io as block_on, git::git};
use tau_vcs_host::{Identity, JjEnv, ProjectRepo, VcsPlugin};

/// A made-up person whose jj config is `config`, and a project imported
/// with their identity from a Git repository holding `files`.
struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
    identity: Identity,
}

impl World {
    fn new(config: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let jj = root.join("config").join("jj");
        std::fs::create_dir_all(&jj).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::write(jj.join("config.toml"), config).unwrap();
        let identity = Identity {
            env: JjEnv {
                config_dir: Some(root.join("config")),
                home_dir: Some(root.join("home")),
                ..JjEnv::default()
            },
            ..Identity::default()
        };
        Self {
            _dir: dir,
            root,
            identity,
        }
    }

    fn project(&self, files: &[(&str, &str)]) -> ProjectRepo {
        let src = self.root.join("src");
        std::fs::create_dir_all(&src).unwrap();
        git(&src, &["init", "--quiet"]);
        for (path, text) in files {
            std::fs::write(src.join(path), text).unwrap();
        }
        git(&src, &["add", "."]);
        git(&src, &["commit", "--quiet", "-m", "first"]);
        ProjectRepo::import(
            src.to_str().unwrap(),
            self.root.join("p"),
            self.identity.clone(),
        )
        .unwrap()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, hegel::DefaultGenerator)]
enum Style {
    Diff,
    Snapshot,
    Git,
}

impl Style {
    fn name(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::Snapshot => "snapshot",
            Self::Git => "git",
        }
    }
}

/// The marker lines `text` holds, each line's first seven characters
/// once `prefix` is taken off: what tells the styles apart.
fn markers(text: &str, prefix: &str) -> Vec<String> {
    let mut found: Vec<String> = text
        .lines()
        .filter_map(|line| line.strip_prefix(prefix))
        .filter_map(|line| line.get(..7))
        .filter(|start| {
            [
                "<<<<<<<", ">>>>>>>", "%%%%%%%", "+++++++", "-------",
                "|||||||",
            ]
            .contains(start)
                || *start == "======="
        })
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found
}

/// What jj writes for a two-sided conflict of one line in each style
/// (jj-lib 0.45.1's `conflicts.rs`, as `docs/conflicts.md` shows them):
/// the markers alone, without labels.
fn expected_markers(style: Style) -> Vec<String> {
    let mut markers: Vec<String> = match style {
        // A diff from the base to one side, then the other side.
        Style::Diff => vec!["<<<<<<<", "%%%%%%%", "+++++++", ">>>>>>>"],
        // Each side whole, and the base.
        Style::Snapshot => vec!["<<<<<<<", "+++++++", "-------", ">>>>>>>"],
        // Git's diff3: one side, the base, the other side.
        Style::Git => vec!["<<<<<<<", "|||||||", "=======", ">>>>>>>"],
    }
    .into_iter()
    .map(str::to_owned)
    .collect();
    markers.sort();
    markers
}

/// A line no marker could start: letters only.
fn line() -> impl hegel::generators::PrintableGenerator<String> {
    gs::from_regex("[a-z]{1,8}").fullmatch(true)
}

/// In every style, the conflicted file a landing writes, `vcs_diff` of
/// the conflicted commit and the project's diff of it carry exactly that
/// style's markers.
#[hegel::test(test_cases = 15)]
fn a_conflict_is_written_and_shown_in_the_configured_style(tc: TestCase) {
    let style = tc.draw(gs::default::<Style>());
    let base = tc.draw(line());
    let ours = tc.draw(line());
    let theirs = tc.draw(line());
    tc.assume(base != ours && base != theirs && ours != theirs);
    let world = World::new(&format!(
        "ui.conflict-marker-style = \"{}\"\n",
        style.name()
    ));
    let project = world.project(&[("a.txt", &format!("{base}\n"))]);
    let trunk = project.trunk().unwrap();
    let parent = project.add_workspace("parent", &trunk).unwrap();
    let child = project.add_workspace("child", &trunk).unwrap();
    std::fs::write(parent.root().join("a.txt"), format!("{ours}\n")).unwrap();
    block_on(parent.commit_all("ours", "tau/parent")).unwrap();
    std::fs::write(child.root().join("a.txt"), format!("{theirs}\n")).unwrap();
    let edit = block_on(child.commit_all("theirs", "tau/child")).unwrap();
    let landing =
        block_on(parent.land(&edit.commit_id, "tau/parent", true)).unwrap();
    assert_eq!(landing.conflicts, ["a.txt"]);
    let expected = expected_markers(style);

    let on_disk = std::fs::read_to_string(parent.root().join("a.txt")).unwrap();
    assert_eq!(markers(&on_disk, ""), expected, "{on_disk}");

    // The landed commit, under `@`.
    let head = landing.changes[0].commit_id.clone();
    let diff = project.diff(&trunk, &head).unwrap();
    assert_eq!(markers(&diff[0].text, "+"), expected, "{}", diff[0].text);

    let tool = VcsPlugin::new(parent.clone())
        .tools()
        .into_iter()
        .find(|tool| tool.name() == "vcs_diff")
        .unwrap();
    let output =
        block_on(tool.call(json!({ "change": head }), ToolCtx::detached()))
            .unwrap();
    let text = output.details.unwrap()["diff"].as_str().unwrap().to_owned();
    assert_eq!(markers(&text, "+"), expected, "{text}");
}

/// Line ends as a checkout writes them: as stored with jj's default,
/// CRLF with `working-copy.eol-conversion = "input-output"`.
#[test]
fn a_checkout_converts_line_ends_as_configured() {
    for (setting, written) in [
        ("", "one\ntwo\n"),
        ("working-copy.eol-conversion = \"none\"\n", "one\ntwo\n"),
        (
            "working-copy.eol-conversion = \"input-output\"\n",
            "one\r\ntwo\r\n",
        ),
    ] {
        let world = World::new(setting);
        let project = world.project(&[("a.txt", "one\ntwo\n")]);
        let run = project
            .add_workspace("run", &project.trunk().unwrap())
            .unwrap();
        let bytes = std::fs::read_to_string(run.root().join("a.txt")).unwrap();
        assert_eq!(bytes, written, "{setting:?}");
    }
}

/// A file made executable is a change, as jj's default sees it on Unix;
/// with `working-copy.exec-bit-change = "ignore"` it is not.
#[cfg(unix)]
#[test]
fn an_executable_bit_change_is_seen_as_configured() {
    use std::os::unix::fs::PermissionsExt as _;
    for (setting, seen) in [
        ("", true),
        ("working-copy.exec-bit-change = \"respect\"\n", true),
        ("working-copy.exec-bit-change = \"ignore\"\n", false),
    ] {
        let world = World::new(setting);
        let project = world.project(&[("run.sh", "echo hi\n")]);
        let run = project
            .add_workspace("run", &project.trunk().unwrap())
            .unwrap();
        let path: &Path = &run.root().join("run.sh");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .unwrap();
        let changes = block_on(run.changes()).unwrap();
        assert_eq!(!changes.is_empty(), seen, "{setting:?}: {changes:?}");
    }
}
