//! tau-memory's imports in Luau (ADR 0044; ADR 0050, gap L), on real
//! folders: what Luau's fake files cannot say, a remote read from a real
//! Git config.

use std::{path::Path, sync::OnceLock};

use serde_json::{Value, json};
use tau_luau_plugins_host::{
    files::Files,
    runtime::{self, Context, Hook, Loaded, NoReach, WithFiles, load},
};
use tau_testing::block_on_io;
use tokio_util::sync::CancellationToken;

/// A package of tau-memory's modules and an entry of the test's own: one
/// tool, `remote`, that reads a folder's remote as the import does,
/// reading anywhere.
const ENTRY: &str = r#"
local tau = require("tau")
local read = require("import_read")

return tau.plugin({
	uses = { files = { { "/", write = true } } },
	tools = {
		remote = {
			description = "the remote of a folder's repository, as the import reads it",
			call = function(args, ctx)
				return { url = read.remote(ctx.files, args.folder) }
			end,
		},
	},
})
"#;

/// The package, loaded once.
fn loaded() -> &'static Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    LOADED.get_or_init(|| {
        let src = tau_luau_plugins::packages::find()
            .unwrap()
            .join("tau-memory/src");
        let mut files = runtime::Files::named("tau-memory-remote");
        files.source = ENTRY.to_owned();
        for entry in std::fs::read_dir(src).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".luau")
                && stem != "plugin"
            {
                files.libs.insert(
                    stem.to_owned(),
                    std::fs::read_to_string(&path).unwrap(),
                );
            }
        }
        block_on_io(load("tau-memory-remote", files)).unwrap()
    })
}

/// What the `remote` tool answers `args` with.
fn call(args: Value) -> Value {
    let loaded = loaded();
    let declared = loaded.declaration.uses.files.clone();
    let files = Files::new(&declared, None, None).writing(&declared);
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
    let started = std::time::Instant::now();
    let outcome = block_on_io(loaded.call(
        &Hook::Tool("remote".into()),
        args,
        &context,
        reach,
        CancellationToken::new(),
    ));
    assert_eq!(
        outcome.error,
        None,
        "after {:?}: {:?}",
        started.elapsed(),
        outcome.logs
    );
    outcome.value
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The remote of the repository `folder` is in, as the Luau reads it.
fn remote_of(folder: &Path) -> Value {
    call(json!({ "folder": folder.display().to_string() }))["url"].clone()
}

/// A remote comes from the config of the repository a folder is in: a
/// checkout's, a worktree's shared one, or a jj repository's Git store;
/// `origin` when there is one, else the first; none without a remote.
#[test]
fn a_folder_remote_comes_from_its_repository_config() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path().canonicalize().unwrap();
    let plain = dir.join("plain");
    write(
        &plain.join(".git/config"),
        "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.com/other.git\n[remote \"origin\"]\n\turl = git@example.com:a/plain.git\n",
    );
    std::fs::create_dir_all(plain.join("deep/er")).unwrap();
    assert_eq!(
        remote_of(&plain.join("deep/er")),
        json!("git@example.com:a/plain.git")
    );
    // A folder that is gone reads as the one above it that is there.
    assert_eq!(
        remote_of(&plain.join("gone/too")),
        json!("git@example.com:a/plain.git")
    );
    // A worktree's `.git` names its directory, which names the shared one.
    let tree = dir.join("tree");
    write(
        &tree.join(".git"),
        &format!("gitdir: {}\n", plain.join(".git/worktrees/tree").display()),
    );
    write(&plain.join(".git/worktrees/tree/commondir"), "../..\n");
    assert_eq!(remote_of(&tree), json!("git@example.com:a/plain.git"));
    // A jj repository: its Git store holds the remote.
    let jj = dir.join("jj");
    write(&jj.join(".jj/repo/store/git_target"), "git");
    write(
        &jj.join(".jj/repo/store/git/config"),
        "[remote \"fork\"]\n\turl = https://e.com/f\n",
    );
    assert_eq!(remote_of(&jj), json!("https://e.com/f"));
    let none = dir.join("none");
    write(&none.join(".git/config"), "[core]\n\tbare = false\n");
    assert_eq!(remote_of(&none), Value::Null);
}
