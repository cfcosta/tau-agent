//! The vcs tools in a chat's workspace while other workspaces act on
//! the same repository (`docs/reference/vcs.md`, "Tools", "Scoping
//! rules", "A repository's main chat").

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, RunId, ToolCtx, ToolUpdates},
};
use tau_ai::message::InputBlock;
use tau_testing::block_on;
use tau_vcs::{
    DEFAULT_WORKSPACE,
    Identity,
    Project,
    UpdateFrom,
    Vcs,
    VcsPlugin,
};
use tokio_util::sync::CancellationToken;

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(["-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn ctx() -> ToolCtx {
    let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
    ToolCtx::new(
        CancellationToken::new(),
        ToolUpdates::for_tests("call_1", sender),
        RunId("run_1".into()),
    )
}

/// One workspace's handle and its tools.
struct Workspace {
    vcs: Vcs,
    tools: Vec<Arc<dyn AgentTool>>,
}

impl Workspace {
    fn new(vcs: Vcs) -> Self {
        let tools = VcsPlugin::new(vcs.clone()).tools();
        Self { vcs, tools }
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
                (text, output.details.unwrap_or(Value::Null))
            })
            .map_err(|err| err.to_string())
    }

    fn ok(&self, name: &str, args: Value) -> (String, Value) {
        let shown = args.to_string();
        self.call(name, args)
            .unwrap_or_else(|err| panic!("{name} {shown} failed: {err}"))
    }
}

/// A project imported from a checkout holding `a.txt`, and its main
/// chat, caught up with trunk as the host has it before its first turn.
fn project() -> (tempfile::TempDir, Project, Workspace) {
    let home = tempfile::tempdir().unwrap();
    let src = home.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    git(&src, &["init", "--quiet"]);
    std::fs::write(src.join("a.txt"), "one\n").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "--quiet", "-m", "first"]);
    let project = Project::import(
        src.to_str().unwrap(),
        home.path().join("p"),
        Identity::default(),
    )
    .unwrap();
    let dir = project.workspace_dir(DEFAULT_WORKSPACE);
    let vcs = Vcs::open(&dir, Identity::default()).unwrap();
    let trunk = project.trunk().unwrap();
    let name = project.trunk_name().unwrap();
    block_on(vcs.move_onto(trunk, name, true)).unwrap();
    (home, project, Workspace::new(vcs))
}

fn assert_refused(result: Result<(String, Value), String>) {
    let err = result.expect_err("undo of an operation the tools did not make");
    assert!(
        err.starts_with("The last operation was not made by the vcs tools"),
        "{err}"
    );
}

// Regressions, pinned as they happened.

/// The main chat's `vcs_undo` before any tool of its own refuses: the
/// newest operation in its workspace is the host's catch-up with trunk.
/// It used to undo it, tagged as a tool's (`move_onto`): `@` went back
/// to the root commit, off trunk, and a catch-up that had restacked the
/// commits chats stand on would have been taken back under them.
#[test]
fn undo_refuses_the_hosts_catch_up() {
    let (_home, project, main) = project();
    let trunk = project.trunk().unwrap();
    assert_refused(main.call("vcs_undo", json!({})));
    let (_, status) = main.ok("vcs_status", json!({}));
    assert_eq!(status["parents"][0]["commit_id"], json!(trunk));
}

/// The same for a landing on the main chat: undoing it would hide the
/// landed changes its links name.
#[test]
fn undo_refuses_a_landing() {
    let (_home, project, main) = project();
    let trunk = project.trunk().unwrap();
    let chat = Workspace::new(project.add_workspace("c", &trunk).unwrap());
    std::fs::write(project.workspace_dir("c").join("b.txt"), "b\n").unwrap();
    let (_, committed) = chat.ok("vcs_commit", json!({ "message": "Add b" }));
    let head = committed["committed"]["commit_id"].as_str().unwrap();
    let name = project.trunk_name().unwrap();
    let landing = block_on(main.vcs.land(head, name, true)).unwrap();
    assert_refused(main.call("vcs_undo", json!({})));
    assert_eq!(project.trunk().unwrap(), landing.head);
}

/// A chat whose `@` holds a conflict, after a catch-up rebased it: the
/// project, the chat, and the chat's workspace directory.
fn a_chat_in_conflict() -> (tempfile::TempDir, Project, Workspace, PathBuf) {
    let (home, project, main) = project();
    let name = project.trunk_name().unwrap();
    std::fs::write(
        project.workspace_dir(DEFAULT_WORKSPACE).join("b.txt"),
        "b\n",
    )
    .unwrap();
    let (_, committed) = main.ok("vcs_commit", json!({ "message": "Add b" }));
    block_on(main.vcs.end_turn(name.clone(), None)).unwrap();
    let head = committed["committed"]["commit_id"].as_str().unwrap();
    let chat = Workspace::new(project.add_workspace("chat", head).unwrap());
    let dir = project.workspace_dir("chat");
    // Upstream changes `a.txt`, and the main chat catches up, while the
    // chat changes it too.
    let src = home.path().join("src");
    std::fs::write(src.join("a.txt"), "three\n").unwrap();
    git(&src, &["commit", "--quiet", "-am", "upstream"]);
    project.update(UpdateFrom::Checkout(&src)).unwrap();
    let trunk = project.trunk().unwrap();
    block_on(main.vcs.move_onto(trunk, name, true)).unwrap();
    std::fs::write(dir.join("a.txt"), "two\n").unwrap();
    let (_, status) = chat.ok("vcs_status", json!({}));
    assert_eq!(status["conflicts"], json!(["a.txt"]));
    (home, project, chat, dir)
}

/// A tool that fails after its snapshot keeps the snapshot: the next
/// tool sees the files as they are. A `vcs_undo` the catch-up made
/// refuse used to leave the working copy's record behind, so the next
/// tool took the snapshot's own changes for edits made since and applied
/// them again: `a.txt`, resolved by hand, came back as a conflict.
#[test]
fn a_refused_tool_keeps_its_snapshot() {
    let (_home, _project, chat, dir) = a_chat_in_conflict();
    std::fs::write(dir.join("a.txt"), "resolved\n").unwrap();
    assert_refused(chat.call("vcs_undo", json!({})));
    let (_, status) = chat.ok("vcs_status", json!({}));
    assert_eq!(status["conflicts"], json!([]));
    assert_eq!(
        std::fs::read_to_string(dir.join("a.txt")).unwrap(),
        "resolved\n"
    );
}
