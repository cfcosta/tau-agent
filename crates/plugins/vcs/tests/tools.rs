//! Every vcs tool end to end, on a real jj repository in a temporary
//! directory (`docs/reference/vcs.md`).

use std::{path::Path, sync::Arc};

use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, ExecutionMode, RunId, ToolCtx, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_vcs::{Identity, Vcs, VcsPlugin};
use tokio_util::sync::CancellationToken;

struct Repo {
    dir: tempfile::TempDir,
    tools: Vec<Arc<dyn AgentTool>>,
}

impl Repo {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
        let tools = VcsPlugin::new(vcs).tools();
        Self { dir, tools }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn write(&self, name: &str, content: &str) {
        let path = self.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.path().join(name)).ok()
    }

    /// Calls `name`; the text and details, or the error's text.
    fn call(&self, name: &str, args: Value) -> Result<(String, Value), String> {
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .unwrap_or_else(|| panic!("no tool {name}"));
        block_on(tool.call(args, ctx()))
            .map(|output| {
                let text = match &output.content[0] {
                    InputBlock::Text(text) => text.text.clone(),
                    other => panic!("expected text, got {other:?}"),
                };
                (text, output.details.unwrap())
            })
            .map_err(|err| err.to_string())
    }

    fn ok(&self, name: &str, args: Value) -> (String, Value) {
        self.call(name, args)
            .unwrap_or_else(|err| panic!("{name} failed: {err}"))
    }
}

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        CancellationToken::new(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

/// The plugin offers the nine tools, read ones first, all sequential;
/// `read_only` keeps the first four.
#[test]
fn the_plugin_offers_every_tool() {
    let dir = tempfile::tempdir().unwrap();
    let vcs = Vcs::init(dir.path(), Identity::default()).unwrap();
    let plugin = VcsPlugin::new(vcs);
    assert_eq!(plugin.name(), "vcs");
    let tools = plugin.tools();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    assert_eq!(
        names,
        [
            "vcs_status",
            "vcs_diff",
            "vcs_log",
            "vcs_show",
            "vcs_describe",
            "vcs_commit",
            "vcs_new",
            "vcs_restore",
            "vcs_undo",
        ]
    );
    for tool in &tools {
        assert_eq!(tool.execution_mode(), ExecutionMode::Sequential);
        assert_eq!(tool.parameters()["type"], json!("object"));
    }
    let read: Vec<String> = plugin
        .read_only()
        .tools()
        .iter()
        .map(|tool| tool.name().to_owned())
        .collect();
    assert_eq!(read, ["vcs_status", "vcs_diff", "vcs_log", "vcs_show"]);
}

/// A fresh repository: an empty working copy on the root commit.
#[test]
fn status_of_a_fresh_repository() {
    let repo = Repo::new();
    let (text, details) = repo.ok("vcs_status", json!({}));
    assert!(text.starts_with("Working copy (@): "), "{text}");
    assert!(text.contains("@ (empty) (no description set)"), "{text}");
    assert!(text.contains("The working copy has no changes."), "{text}");
    assert_eq!(details["changes"], json!([]));
    assert_eq!(details["parents"][0]["immutable"], json!(true));
}

/// Files written by other tools show up without staging, with their
/// kind, and deleted files as `D`.
#[test]
fn status_shows_written_files() {
    let repo = Repo::new();
    repo.write("hello.txt", "hello\n");
    repo.write("src/lib.rs", "fn main() {}\n");
    let (text, details) = repo.ok("vcs_status", json!({}));
    assert!(text.contains("Working copy changes:\nA hello.txt\nA src/lib.rs"));
    assert_eq!(
        details["changes"],
        json!([
            {"path": "hello.txt", "kind": "added"},
            {"path": "src/lib.rs", "kind": "added"},
        ])
    );
    assert_eq!(details["working_copy"]["empty"], json!(false));
    // Details hold @'s diff for a caller to draw; the text leaves it
    // to vcs_diff.
    let (diff, _) = repo.ok("vcs_diff", json!({}));
    assert_eq!(details["diff"].as_str(), Some(diff.as_str()));
    assert_eq!(details["truncated"], json!(false));
    assert!(!text.contains("diff --git"), "{text}");

    repo.ok("vcs_commit", json!({"message": "Add files"}));
    std::fs::remove_file(repo.path().join("hello.txt")).unwrap();
    repo.write("src/lib.rs", "fn main() { todo!() }\n");
    let (text, _) = repo.ok("vcs_status", json!({}));
    assert!(text.contains("D hello.txt\nM src/lib.rs"), "{text}");
    assert!(text.contains("Parent (@-):      "), "{text}");
    assert!(text.contains("Add files"), "{text}");
}

/// Every new file is tracked except one over 1 MiB, which status names
/// with its size and which stays out of `@`.
#[test]
fn status_names_files_too_large_to_snapshot() {
    let repo = Repo::new();
    repo.write("small.txt", "small\n");
    let big = "x".repeat(tau_vcs::MAX_NEW_FILE_SIZE as usize + 1);
    repo.write("big.bin", &big);
    let (text, details) = repo.ok("vcs_status", json!({}));
    assert!(
        text.contains(
            "Left out of @ (new files over 1 MiB are not snapshotted):\n\
             big.bin (1.0 MiB)"
        ),
        "{text}"
    );
    assert!(!text.contains("Not tracked"), "{text}");
    assert_eq!(
        details["too_large"],
        json!([{"path": "big.bin", "size": big.len()}])
    );
    assert_eq!(
        details["changes"],
        json!([{"path": "small.txt", "kind": "added"}])
    );
}

/// `vcs_commit` describes the change and starts an empty one on top;
/// `vcs_log` lists both, newest first, with the ids the tools accept.
#[test]
fn commit_creates_a_described_change_that_log_lists() {
    let repo = Repo::new();
    repo.write("hello.txt", "hello\n");
    let (text, details) =
        repo.ok("vcs_commit", json!({"message": "Add hello\n\nBody.\n"}));
    assert!(text.starts_with("Committed change "), "{text}");
    assert!(text.contains("Add hello"), "{text}");
    assert_eq!(
        details["committed"]["description"],
        json!("Add hello\n\nBody.\n")
    );
    assert_eq!(details["committed"]["working_copy"], json!(false));
    assert_eq!(details["working_copy"]["empty"], json!(true));
    let committed = details["committed"]["change_id"].as_str().unwrap();

    let (text, details) = repo.ok("vcs_log", json!({}));
    let rows: Vec<&str> = text.lines().collect();
    assert_eq!(rows.len(), 2, "{text}");
    assert!(
        rows[0].ends_with("@ (empty) (no description set)"),
        "{text}"
    );
    assert!(rows[1].ends_with(" Add hello"), "{text}");
    assert!(rows[1].starts_with(&committed[..12]), "{text}");
    assert_eq!(details["changes"][1]["change_id"], json!(committed));
    assert_eq!(details["more"], json!(false));

    // The file is still there, and the new change has nothing yet.
    assert_eq!(repo.read("hello.txt").as_deref(), Some("hello\n"));
    let (_, details) = repo.ok("vcs_status", json!({}));
    assert_eq!(details["changes"], json!([]));
}

/// An empty message is refused.
#[test]
fn commit_needs_a_message() {
    let repo = Repo::new();
    let err = repo
        .call("vcs_commit", json!({"message": "  "}))
        .unwrap_err();
    assert_eq!(err, "The description must not be empty");
}

/// `limit` caps the rows and says how to see more.
#[test]
fn log_limit() {
    let repo = Repo::new();
    for n in 0..3 {
        repo.write("n.txt", &n.to_string());
        repo.ok("vcs_commit", json!({"message": format!("Change {n}")}));
    }
    let (text, details) = repo.ok("vcs_log", json!({"limit": 2}));
    assert!(text.contains("Change 2"), "{text}");
    assert!(!text.contains("Change 1"), "{text}");
    assert!(
        text.ends_with("[Showing the newest 2 changes. Use limit=4 for more]"),
        "{text}"
    );
    assert_eq!(details["more"], json!(true));
    assert_eq!(details["changes"].as_array().unwrap().len(), 2);
}

/// `vcs_diff` shows the working copy's hunks by default, a given
/// change's otherwise, and only the paths asked for.
#[test]
fn diff_shows_the_hunks() {
    let repo = Repo::new();
    repo.write("a.txt", "one\ntwo\n");
    let (_, details) = repo.ok("vcs_commit", json!({"message": "Add a"}));
    let first = details["committed"]["change_id"]
        .as_str()
        .unwrap()
        .to_owned();
    repo.write("a.txt", "one\nthree\n");
    repo.write("b.txt", "bee\n");

    let (text, details) = repo.ok("vcs_diff", json!({}));
    assert!(
        text.contains(
            "diff --git a/a.txt b/a.txt\n--- a/a.txt\n+++ b/a.txt\n\
             @@ -1,2 +1,2 @@\n one\n-two\n+three\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "diff --git a/b.txt b/b.txt\nnew file mode 100644\n\
             --- /dev/null\n+++ b/b.txt\n@@ -0,0 +1 @@\n+bee\n"
        ),
        "{text}"
    );
    assert_eq!(
        details["files"],
        json!([
            {"path": "a.txt", "kind": "modified"},
            {"path": "b.txt", "kind": "added"},
        ])
    );
    // Details hold the same diff, for a caller to draw.
    assert_eq!(details["diff"].as_str(), Some(text.as_str()));
    assert_eq!(details["truncated"], json!(false));

    let (text, _) = repo.ok("vcs_diff", json!({"paths": ["b.txt"]}));
    assert!(!text.contains("a.txt"), "{text}");
    assert!(text.contains("+bee"), "{text}");

    let (text, details) = repo.ok("vcs_diff", json!({"change": first}));
    assert!(text.contains("+one\n+two\n"), "{text}");
    assert_eq!(details["change"]["description"], json!("Add a\n"));

    // A commit id prefix works as well as a change id.
    let commit = details["change"]["commit_id"].as_str().unwrap();
    let (by_commit, _) = repo.ok("vcs_diff", json!({"change": &commit[..8]}));
    assert_eq!(by_commit, text);
}

/// A change with no diff says so.
#[test]
fn diff_of_an_empty_change() {
    let repo = Repo::new();
    let (text, details) = repo.ok("vcs_diff", json!({}));
    assert!(text.starts_with("No changes in "), "{text}");
    assert_eq!(details["diff"], json!(""));
}

/// A diff past 50 KB is cut at a line, with a note for the model that
/// the details leave out.
#[test]
fn a_long_diff_is_cut() {
    let repo = Repo::new();
    let line = "x".repeat(99);
    repo.write("big.txt", &format!("{line}\n").repeat(600));
    let (text, details) = repo.ok("vcs_diff", json!({}));
    assert!(
        text.ends_with("a few files at a time.]"),
        "{}",
        &text[text.len() - 80..]
    );
    assert_eq!(details["truncated"], json!(true));
    let diff = details["diff"].as_str().unwrap();
    assert!(diff.len() <= tau_vcs::MAX_DIFF_BYTES);
    assert!(diff.ends_with(&format!("+{line}\n")));
    assert!(text.starts_with(diff));
}

/// `vcs_show` gives the ids, author, full description and the diff.
#[test]
fn show_a_change() {
    let repo = Repo::new();
    repo.write("a.txt", "alpha\n");
    let (_, details) = repo.ok(
        "vcs_commit",
        json!({"message": "Add alpha\n\nIt was missing."}),
    );
    let change = details["committed"]["change_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (text, details) = repo.ok("vcs_show", json!({"change": &change[..6]}));
    assert!(
        text.starts_with(&format!("Change ID: {change}\n")),
        "{text}"
    );
    assert!(text.contains("Author: tau <tau@localhost>"), "{text}");
    assert!(
        text.contains("\n    Add alpha\n    \n    It was missing.\n"),
        "{text}"
    );
    assert!(
        text.contains("+++ b/a.txt\n@@ -0,0 +1 @@\n+alpha"),
        "{text}"
    );
    assert_eq!(details["author"]["name"], json!("tau"));
    assert_eq!(details["parents"][0]["immutable"], json!(true));
    let diff = details["diff"].as_str().unwrap();
    assert!(diff.starts_with("diff --git a/a.txt b/a.txt\n"), "{diff}");
    assert!(text.ends_with(diff.trim_end()), "{text}");
}

/// The root commit is shown as immutable.
#[test]
fn show_the_root_commit() {
    let repo = Repo::new();
    let (text, details) = repo.ok("vcs_show", json!({"change": "zzzzzzzz"}));
    assert!(text.contains("Flags: empty, immutable"), "{text}");
    assert_eq!(details["change"]["immutable"], json!(true));
}

/// Only ids are accepted: revsets, names and unknown ids are refused.
#[test]
fn only_ids_are_accepted() {
    let repo = Repo::new();
    for rev in ["@", "@-", "all()", "main", "trunk()", ""] {
        let err = repo.call("vcs_show", json!({"change": rev})).unwrap_err();
        assert!(
            err.contains("is not a change id or a commit id"),
            "{rev}: {err}"
        );
    }
    let err = repo
        .call("vcs_show", json!({"change": "kkkkkkkk"}))
        .unwrap_err();
    assert_eq!(err, "No change matches `kkkkkkkk`");
    let err = repo
        .call("vcs_diff", json!({"change": "0123abcd"}))
        .unwrap_err();
    assert_eq!(err, "No commit matches `0123abcd`");
}

/// `vcs_describe` changes the working copy's description and keeps
/// working in the same change.
#[test]
fn describe_the_working_copy() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let (_, before) = repo.ok("vcs_status", json!({}));
    let (text, details) =
        repo.ok("vcs_describe", json!({"message": "Work in progress"}));
    assert!(text.starts_with("Described the working copy.\n"), "{text}");
    assert!(text.ends_with(" @ Work in progress"), "{text}");
    assert_eq!(
        details["working_copy"]["change_id"],
        before["working_copy"]["change_id"]
    );
    assert_eq!(
        details["working_copy"]["description"],
        json!("Work in progress\n")
    );
    let (text, _) = repo.ok("vcs_log", json!({}));
    assert_eq!(text.lines().count(), 1, "{text}");
}

/// `vcs_new` starts an empty change on top; files stay as they are.
#[test]
fn new_starts_an_empty_change() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    let (_, before) = repo.ok("vcs_status", json!({}));
    let (text, details) = repo.ok("vcs_new", json!({"message": "Next step"}));
    assert!(text.starts_with("Started a new change.\n"), "{text}");
    assert_eq!(details["working_copy"]["empty"], json!(true));
    assert_eq!(details["working_copy"]["description"], json!("Next step\n"));
    assert_ne!(
        details["working_copy"]["change_id"],
        before["working_copy"]["change_id"]
    );
    assert_eq!(repo.read("a.txt").as_deref(), Some("a\n"));
    let (_, log) = repo.ok("vcs_log", json!({}));
    assert_eq!(
        log["changes"][1]["change_id"],
        before["working_copy"]["change_id"]
    );
}

/// `vcs_restore` puts paths back as the parent has them: modified files
/// revert, added files go, and paths not named are left alone.
#[test]
fn restore_reverts_paths() {
    let repo = Repo::new();
    repo.write("a.txt", "original\n");
    repo.ok("vcs_commit", json!({"message": "Add a"}));
    repo.write("a.txt", "changed\n");
    repo.write("b.txt", "new\n");

    let (text, details) = repo.ok("vcs_restore", json!({"paths": ["a.txt"]}));
    assert!(
        text.starts_with("Restored:\na.txt\nWorking copy (@): "),
        "{text}"
    );
    assert_eq!(
        details["restored"],
        json!([{"path": "a.txt", "kind": "modified"}])
    );
    assert_eq!(repo.read("a.txt").as_deref(), Some("original\n"));
    assert_eq!(repo.read("b.txt").as_deref(), Some("new\n"));

    repo.ok("vcs_restore", json!({"paths": ["./b.txt"]}));
    assert_eq!(repo.read("b.txt"), None);
    let (_, status) = repo.ok("vcs_status", json!({}));
    assert_eq!(status["changes"], json!([]));

    let (text, _) = repo.ok("vcs_restore", json!({"paths": ["."]}));
    assert!(text.starts_with("Nothing to restore"), "{text}");
}

/// `from` takes the files from another change instead of the parent.
#[test]
fn restore_from_another_change() {
    let repo = Repo::new();
    repo.write("a.txt", "v1\n");
    let (_, first) = repo.ok("vcs_commit", json!({"message": "v1"}));
    repo.write("a.txt", "v2\n");
    repo.ok("vcs_commit", json!({"message": "v2"}));
    let from = first["committed"]["change_id"].as_str().unwrap();
    repo.ok("vcs_restore", json!({"paths": ["a.txt"], "from": from}));
    assert_eq!(repo.read("a.txt").as_deref(), Some("v1\n"));
}

/// Paths outside the repository are refused.
#[test]
fn restore_refuses_paths_outside() {
    let repo = Repo::new();
    let err = repo
        .call("vcs_restore", json!({"paths": ["../x"]}))
        .unwrap_err();
    assert_eq!(err, "`../x` is not a path inside the repository");
    let err = repo
        .call("vcs_restore", json!({"paths": ["/etc/passwd"]}))
        .unwrap_err();
    assert_eq!(err, "`/etc/passwd` is outside the repository");
    let err = repo.call("vcs_restore", json!({"paths": []})).unwrap_err();
    assert!(err.starts_with("Name at least one path"), "{err}");
}

/// `vcs_undo` undoes the tools' own operations, newest first, and
/// keeps the files; it refuses operations it did not make.
#[test]
fn undo_undoes_the_last_operation() {
    let repo = Repo::new();
    let err = repo.call("vcs_undo", json!({})).unwrap_err();
    assert!(
        err.starts_with("The last operation was not made by the vcs tools"),
        "{err}"
    );

    repo.write("hello.txt", "hello\n");
    repo.ok("vcs_describe", json!({"message": "First"}));
    repo.ok("vcs_commit", json!({"message": "Second"}));
    let (text, _) = repo.ok("vcs_log", json!({}));
    assert_eq!(text.lines().count(), 2, "{text}");

    let (text, details) = repo.ok("vcs_undo", json!({}));
    assert!(text.starts_with("Undid operation "), "{text}");
    assert!(text.contains("(vcs_commit)"), "{text}");
    assert_eq!(details["tool"], json!("commit"));
    assert_eq!(details["working_copy"]["description"], json!("First\n"));
    let (text, _) = repo.ok("vcs_log", json!({}));
    assert_eq!(text.lines().count(), 1, "{text}");
    assert_eq!(repo.read("hello.txt").as_deref(), Some("hello\n"));
    let (_, status) = repo.ok("vcs_status", json!({}));
    assert_eq!(
        status["changes"],
        json!([{"path": "hello.txt", "kind": "added"}])
    );

    // Again: the describe goes too.
    let (_, details) = repo.ok("vcs_undo", json!({}));
    assert_eq!(details["tool"], json!("describe"));
    assert_eq!(details["working_copy"]["description"], json!(""));
    assert_eq!(repo.read("hello.txt").as_deref(), Some("hello\n"));

    // Nothing of ours is left.
    let err = repo.call("vcs_undo", json!({})).unwrap_err();
    assert!(
        err.starts_with("The last operation was not made by the vcs tools"),
        "{err}"
    );
}

/// Undoing a restore brings the discarded edit back.
#[test]
fn undo_a_restore() {
    let repo = Repo::new();
    repo.write("a.txt", "original\n");
    repo.ok("vcs_commit", json!({"message": "Add a"}));
    repo.write("a.txt", "edited\n");
    repo.ok("vcs_restore", json!({"paths": ["a.txt"]}));
    assert_eq!(repo.read("a.txt").as_deref(), Some("original\n"));
    let (_, details) = repo.ok("vcs_undo", json!({}));
    assert_eq!(details["tool"], json!("restore"));
    assert_eq!(repo.read("a.txt").as_deref(), Some("edited\n"));
}

/// A second `Vcs` opened on the same directory sees the history.
#[test]
fn open_an_existing_repository() {
    let repo = Repo::new();
    repo.write("a.txt", "a\n");
    repo.ok("vcs_commit", json!({"message": "Add a"}));
    let vcs = Vcs::open(repo.path(), Identity::default()).unwrap();
    let tools = VcsPlugin::new(vcs).tools();
    let log = tools.iter().find(|tool| tool.name() == "vcs_log").unwrap();
    let output = block_on(log.call(json!({}), ctx())).unwrap();
    let details = output.details.unwrap();
    assert_eq!(details["changes"][1]["description"], json!("Add a\n"));
}

/// A cancelled run gets the shared abort message before any work.
#[test]
fn cancelled_before_starting() {
    let repo = Repo::new();
    let tool = repo
        .tools
        .iter()
        .find(|tool| tool.name() == "vcs_status")
        .unwrap();
    let ctx = ctx();
    ctx.cancel.cancel();
    let err = block_on(tool.call(json!({}), ctx)).unwrap_err();
    assert_eq!(err.to_string(), tau_vcs::ABORTED);
}
