//! What a Luau plugin writes, and what tau-memory's port needs besides
//! (ADR 0050, gap L): the folders its `uses.files` names `{ name, write
//! = true }`, written whole and nowhere past them; its own folders in
//! tau's, which need no Allow; a folder's Git remote; `infer` offering
//! the plugin's own tools; and the commit a turn leaves, in `turn_end`.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};
use tau_luau_plugins_host::{files::Files, remote, runtime};
use tau_testing::block_on_io;

/// A folder to write, `notes`, one beside it only read, `shelf`, and one
/// outside both, with a link to it from `notes`.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        std::fs::create_dir_all(path.join("notes")).unwrap();
        std::fs::create_dir_all(path.join("shelf")).unwrap();
        std::fs::write(path.join("shelf/book.md"), "a book").unwrap();
        std::fs::create_dir(path.join("outside")).unwrap();
        std::fs::write(path.join("outside/secret.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(
            path.join("outside"),
            path.join("notes/away"),
        )
        .unwrap();
        Self { dir }
    }

    fn path(&self, rest: &str) -> String {
        self.dir.path().join(rest).display().to_string()
    }

    /// `notes` written, `shelf` read only.
    fn files(&self) -> Files {
        let notes = self.path("notes");
        let shelf = self.path("shelf");
        Files::new(&[notes.clone(), shelf], None, None).writing(&[notes])
    }
}

fn call(files: &Files, name: &str, args: Value) -> Result<Value, String> {
    block_on_io(files.call(name, &args)).map(|reply| reply.value)
}

/// A write lands whole, the folders it is in made, nothing left beside
/// it; it reads back, moves, and goes.
#[test]
fn a_plugin_writes_its_folders() {
    let f = Fixture::new();
    let files = f.files();
    let note = f.path("notes/fact/a.md");
    assert_eq!(
        call(
            &files,
            "files_write",
            json!({ "path": note, "text": "one" })
        ),
        Ok(Value::Null)
    );
    assert_eq!(
        call(
            &files,
            "files_write",
            json!({ "path": note, "text": "two" })
        ),
        Ok(Value::Null)
    );
    assert_eq!(std::fs::read_to_string(&note).unwrap(), "two");
    assert_eq!(
        call(&files, "files_read", json!({ "path": note })),
        Ok(json!("two"))
    );
    let left: Vec<String> = std::fs::read_dir(f.path("notes/fact"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into())
        .collect();
    assert_eq!(left, ["a.md"]);
    let moved = f.path("notes/history/a/1.md");
    assert_eq!(
        call(&files, "files_rename", json!({ "path": note, "to": moved })),
        Ok(Value::Null)
    );
    assert!(!Path::new(&note).exists());
    assert_eq!(std::fs::read_to_string(&moved).unwrap(), "two");
    assert_eq!(
        call(
            &files,
            "files_mkdir",
            json!({ "path": f.path("notes/x/y") })
        ),
        Ok(Value::Null)
    );
    assert!(Path::new(&f.path("notes/x/y")).is_dir());
    assert_eq!(
        call(&files, "files_remove", json!({ "path": moved })),
        Ok(Value::Null)
    );
    assert!(!Path::new(&moved).exists());
    assert_eq!(
        call(
            &files,
            "files_remove",
            json!({ "path": f.path("notes/x/y") })
        ),
        Ok(Value::Null)
    );
}

/// Nothing is written past the folders it writes: not one it only reads,
/// nor through `..` or a link, nor the folder itself, nor a file past
/// the most a read takes.
#[test]
fn nothing_past_its_written_folders_is_written() {
    let f = Fixture::new();
    let files = f.files();
    for path in [
        f.path("shelf/new.md"),
        f.path("shelf/book.md"),
        f.path("notes/../outside/new.txt"),
        f.path("notes/../notes/../outside/new.txt"),
        f.path("notes/away/new.txt"),
        f.path("notes/away/secret.txt"),
        f.path("outside/secret.txt"),
        "/tmp/anywhere.txt".to_owned(),
    ] {
        let error =
            call(&files, "files_write", json!({ "path": path, "text": "x" }))
                .unwrap_err();
        assert!(error.contains("does not write"), "{path}: {error}");
    }
    assert_eq!(
        std::fs::read_to_string(f.path("outside/secret.txt")).unwrap(),
        "secret"
    );
    assert!(!Path::new(&f.path("outside/new.txt")).exists());
    let error = call(
        &files,
        "files_remove",
        json!({ "path": f.path("shelf/book.md") }),
    )
    .unwrap_err();
    assert!(error.contains("does not write"), "{error}");
    let error = call(
        &files,
        "files_rename",
        json!({ "path": f.path("shelf/book.md"), "to": f.path("notes/b.md") }),
    )
    .unwrap_err();
    assert!(error.contains("does not write"), "{error}");
    let error =
        call(&files, "files_remove", json!({ "path": f.path("notes") }))
            .unwrap_err();
    assert!(error.contains("not one in it"), "{error}");
    let big = "x".repeat(tau_luau_plugins_host::files::MAX_BYTES as usize + 1);
    let error = call(
        &files,
        "files_write",
        json!({ "path": f.path("notes/big.txt"), "text": big }),
    )
    .unwrap_err();
    assert!(error.contains("at most"), "{error}");
    // A folder only read is written by no one.
    let read_only =
        Files::new(&[f.path("notes")], None, None).writing(&[f.path("shelf")]);
    let error = call(
        &read_only,
        "files_write",
        json!({ "path": f.path("shelf/new.md"), "text": "x" }),
    )
    .unwrap_err();
    assert!(error.contains("does not write"), "{error}");
}

/// `repository` is tau's folder for the run's repository, and a hook
/// outside its runs finds each repository's by name.
#[test]
fn the_repository_is_taus_folder_for_it() {
    let f = Fixture::new();
    let repo = f.dir.path().join("repos/a");
    let files = Files::new(&["repository/p".to_owned()], None, None)
        .writing(&["repository/p".to_owned()])
        .with_repository(Some(repo.clone()))
        .with_repositories(vec![("a".into(), repo.clone())]);
    let note = repo.join("p/note.md").display().to_string();
    assert_eq!(
        call(&files, "files_write", json!({ "path": note, "text": "n" })),
        Ok(Value::Null)
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("p/note.md")).unwrap(),
        "n"
    );
    assert_eq!(
        call(&files, "files_roots", json!({})),
        Ok(json!({ "repository/p": repo.join("p").display().to_string() }))
    );
    assert_eq!(
        call(&files, "files_repository", json!({ "name": "a" })),
        Ok(json!(repo.display().to_string()))
    );
    assert_eq!(
        call(&files, "files_repository", json!({ "name": "b" })),
        Ok(Value::Null)
    );
    // Beside its own folder, nothing.
    let error = call(
        &files,
        "files_write",
        json!({ "path": repo.join("rules.md").display().to_string(), "text": "x" }),
    )
    .unwrap_err();
    assert!(error.contains("does not write"), "{error}");
}

/// Outside any repository, a `repository/...` folder is that folder of
/// each repository tau has, written as its own; and the repositories tau
/// lists are there for a plugin that keeps per repository (ADR 0050, gap
/// L).
#[test]
fn outside_a_repository_its_folder_is_each_repositorys() {
    let f = Fixture::new();
    let a = f.dir.path().join("repos/a");
    let b = f.dir.path().join("repos/b");
    let files = Files::new(&["repository/p".to_owned()], None, None)
        .writing(&["repository/p".to_owned()])
        .with_repositories(vec![
            ("a".into(), a.clone()),
            ("b".into(), b.clone()),
        ]);
    for dir in [&a, &b] {
        let note = dir.join("p/note.md").display().to_string();
        assert_eq!(
            call(&files, "files_write", json!({ "path": note, "text": "n" })),
            Ok(Value::Null)
        );
    }
    assert_eq!(std::fs::read_to_string(b.join("p/note.md")).unwrap(), "n");
    // Beside each one's own folder, nothing.
    let error = call(
        &files,
        "files_write",
        json!({ "path": a.join("rules.md").display().to_string(), "text": "x" }),
    )
    .unwrap_err();
    assert!(error.contains("does not write"), "{error}");
    // No one repository is its `repository` there.
    assert_eq!(call(&files, "files_roots", json!({})), Ok(json!({})));

    let listed = files.with_listed(&[tau_ui_plugin::RepoCtx {
        id: tau_ui_plugin::RepoId("id-a".into()),
        name: "a".into(),
        remote: Some("https://github.com/Acme/A.git".into()),
        folder: Some(f.dir.path().join("work/a")),
        checkout: a.clone(),
        dir: a.clone(),
        workspaces: a.clone(),
    }]);
    assert_eq!(
        call(&listed, "files_repositories", json!({})),
        Ok(json!([{
            "id": "id-a",
            "name": "a",
            "folder": f.path("work/a"),
            "remote": "https://github.com/Acme/A.git",
            "normalized": "github.com/Acme/A",
        }]))
    );
    // A plugin that keeps nothing per repository lists none.
    let other = Files::new(&["data/p".to_owned()], None, None);
    let error = call(&other, "files_repositories", json!({})).unwrap_err();
    assert!(error.contains("needs `repository`"), "{error}");
}

/// Writing its own folders in tau's is no new reach; any other folder it
/// writes waits for an Allow of its own, apart from reading it.
#[test]
fn only_a_folder_not_its_own_waits_to_be_written() {
    let read = tau_luau_plugins::Uses {
        files: vec!["~/notes".into(), "data/p".into(), "repository/p/x".into()],
        ..Default::default()
    };
    let written = tau_luau_plugins::Uses {
        writes: read.files.clone(),
        ..read.clone()
    };
    assert_eq!(
        written.grown_from(&read, "p"),
        ["writing files in `~/notes`"]
    );
    assert_eq!(
        written.grown_from(&read, "q"),
        [
            "writing files in `~/notes`",
            "writing `p` in tau's data folder",
            "writing `p/x` in tau's folder for the repository",
        ]
    );
    assert!(written.grown_from(&written, "q").is_empty());
    assert!(tau_luau_plugins::is_own_folder("p", "data/p"));
    assert!(tau_luau_plugins::is_own_folder("p", "repository/p/notes"));
    assert!(!tau_luau_plugins::is_own_folder("p", "data/pq"));
    assert!(!tau_luau_plugins::is_own_folder("p", "data"));
}

/// `ctx.files.remote` reads the remote of the repository a folder is in,
/// a checkout, a worktree or a jj repository, `origin` first, and
/// normalises it; a folder in none has none.
#[test]
fn a_folder_has_its_repositorys_remote() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let config = "[core]\n\tbare = false\n[remote \"upstream\"]\n\turl = https://example.com/u/v\n[remote \"origin\"]\n\turl = git@GitHub.com:o/n.git\n";
    std::fs::create_dir_all(root.join("checkout/.git")).unwrap();
    std::fs::create_dir_all(root.join("checkout/src/deep")).unwrap();
    std::fs::write(root.join("checkout/.git/config"), config).unwrap();
    // A worktree: `.git` a file naming its Git directory, which names the
    // shared one.
    std::fs::create_dir_all(root.join("checkout/.git/worktrees/w")).unwrap();
    std::fs::write(root.join("checkout/.git/worktrees/w/commondir"), "../..")
        .unwrap();
    std::fs::create_dir_all(root.join("tree")).unwrap();
    std::fs::write(
        root.join("tree/.git"),
        format!(
            "gitdir: {}\n",
            root.join("checkout/.git/worktrees/w").display()
        ),
    )
    .unwrap();
    // A jj repository, its Git store where `git_target` says.
    std::fs::create_dir_all(root.join("jj/.jj/repo/store")).unwrap();
    std::fs::create_dir_all(root.join("jj/.jj/repo/store/git")).unwrap();
    std::fs::write(root.join("jj/.jj/repo/store/git_target"), "git").unwrap();
    std::fs::write(
        root.join("jj/.jj/repo/store/git/config"),
        "[remote \"mirror\"]\n\turl = ssh://git@host.example:22/a/b/\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("plain")).unwrap();
    let files = Files::new(&[root.display().to_string()], None, None);
    let remote_of = |folder: &str| {
        call(
            &files,
            "files_remote",
            json!({ "path": root.join(folder).display().to_string() }),
        )
        .unwrap()
    };
    let origin = json!({
        "url": "git@GitHub.com:o/n.git",
        "normalized": "github.com/o/n",
    });
    assert_eq!(remote_of("checkout/src/deep"), origin);
    assert_eq!(remote_of("tree"), origin);
    assert_eq!(
        remote_of("jj"),
        json!({
            "url": "ssh://git@host.example:22/a/b/",
            "normalized": "host.example/a/b",
        })
    );
    // `plain` is in the temporary folder, which may itself be in a
    // checkout: none is found below the fixture's own folder.
    let plain = remote_of("plain");
    assert!(
        plain.is_null()
            || !plain["url"]
                .as_str()
                .is_some_and(|url| url.contains("example")),
        "{plain}"
    );
    // A folder it does not read has no remote it may see.
    let error = call(
        &Files::new(&["~/elsewhere".to_owned()], None, None),
        "files_remote",
        json!({ "path": root.join("checkout").display().to_string() }),
    )
    .unwrap_err();
    assert!(error.contains("does not read"), "{error}");
    assert_eq!(
        remote::normalize("https://GitHub.com/o/n.git/"),
        Some("github.com/o/n".into())
    );
    assert_eq!(remote::normalize("/srv/repo.git"), Some("/srv/repo".into()));
    assert_eq!(remote::normalize("  "), None);
}

/// A plugin whose tools write notes, whose `turn_end` keeps the turn's
/// commit, and whose `run_end` asks the model with its own tools.
const PLUGIN: &str = r#"
local tau = require("tau")

local function root(ctx)
	return ctx.files.roots()["data/scribe"]
end

return tau.plugin({
	uses = { infer = true, files = { { "data/scribe", write = true }, "repository/scribe" } },
	tools = {
		note_search = {
			description = "Finds notes.",
			parameters = { type = "object", properties = { query = { type = "string" } } },
			call = function(args, ctx)
				ctx.state.searched = (ctx.state.searched or 0) + 1
				return "nothing about " .. tostring(args.query)
			end,
		},
		note_write = {
			description = "Writes a note.",
			parameters = { type = "object", properties = { text = { type = "string" } } },
			call = function(args, ctx)
				ctx.files.write(root(ctx) .. "/notes/" .. args.name .. ".md", args.text)
				return tau.result("written " .. args.name)
			end,
		},
		note_fail = {
			description = "Fails.",
			call = function()
				error("it broke", 0)
			end,
		},
	},
	turn_end = function(turn, ctx)
		if turn.commit then
			ctx.files.write(root(ctx) .. "/commits/" .. turn.turn .. ".json", json.encode(turn.commit))
		end
	end,
	run_end = function(_, ctx)
		local ok, refused = pcall(ctx.tools.infer, { task = "x", tools = { "exec_command" } })
		ctx.state.refused = not ok and tostring(refused) or nil
		local answer = ctx.tools.infer({
			task = "Keep what matters.",
			instructions = "You keep notes.",
			tools = { "note_search", "note_write", "note_fail" },
			wait_for = { "note_search" },
		})
		ctx.state.answer = answer
	end,
})
"#;

fn package() -> runtime::Files {
    runtime::Files {
        source: PLUGIN.into(),
        libs: BTreeMap::new(),
        ..runtime::Files::named("scribe")
    }
}

/// `infer{ tools }` offers the plugin's own tools and none other, calls
/// each the model asks for with the hook's `ctx`, gives back what each
/// answered, and stops at a round that waits for none; `turn_end` hears
/// the turn's commit.
#[tokio::test(flavor = "multi_thread")]
async fn infer_calls_the_plugins_own_tools_and_turn_end_hears_the_commit() {
    let spec = json!({
        "prompt": "Do it.",
        "model": [
            { "text": "Done." },
            [
                { "tool": "note_search", "args": { "query": "retries" } },
                { "tool": "note_write", "args": { "name": "early", "text": "too soon" } },
            ],
            [
                { "tool": "note_write", "args": { "name": "retry", "text": "Retries back off." } },
                { "tool": "note_fail", "args": {} },
                { "tool": "exec_command", "args": {} },
            ],
            { "text": "never asked" },
        ],
        "commits": [{ "turn": 1, "change": "kxyz", "paths": ["src/a.rs"] }],
    });
    let came = tau_luau_plugins_host::looping::run(&package(), &spec)
        .await
        .unwrap();
    let state = &came["state"];
    assert!(
        state["refused"]
            .as_str()
            .is_some_and(|why| why.contains("no `exec_command`")),
        "{state}"
    );
    let answer = &state["answer"];
    assert_eq!(answer["ok"], json!(true), "{answer}");
    assert_eq!(state["searched"], json!(1));
    let calls: Vec<(&str, bool)> = answer["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| (call["name"].as_str().unwrap(), call["ok"] == json!(true)))
        .collect();
    assert_eq!(
        calls,
        [
            ("note_search", true),
            ("note_write", true),
            ("note_write", true),
            ("note_fail", false),
            ("exec_command", false),
        ]
    );
    let data = &came["data"];
    assert_eq!(data["scribe/notes/early.md"], json!("too soon"), "{data}");
    assert_eq!(data["scribe/notes/retry.md"], json!("Retries back off."));
    let commit: Value =
        serde_json::from_str(data["scribe/commits/1.json"].as_str().unwrap())
            .unwrap();
    assert_eq!(commit, json!({ "change": "kxyz", "paths": ["src/a.rs"] }));
    // The run's request, then the two rounds; the third answer is never
    // asked for, as the second round searched nothing.
    let requests = came["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 3, "{requests:#?}");
    let round = &requests[2];
    assert_eq!(
        round["settings"]["instructions"],
        json!("You keep notes."),
        "{round}"
    );
    assert_eq!(
        round["settings"]["tools"],
        json!(["note_search", "note_write", "note_fail"])
    );
    let shown = round["messages"].to_string();
    assert!(shown.contains("nothing about retries"), "{shown}");
    assert!(shown.contains("written early"), "{shown}");
}

/// A plugin's own tests fake all of it: `spec.infer`'s answers, round by
/// round, what the plugin writes back in `run.files`, `spec.remotes` and
/// `spec.repositories`; writing needs a folder it writes.
#[test]
fn the_harness_fakes_writes_remotes_and_rounds() {
    let tests = r#"
local t = require("tau").test

t.case("rounds call the plugin's own tools and write its files", function()
	local run = t.run({
		roots = { ["data/scribe"] = "d" },
		infer = {
			{ calls = { { name = "note_search", args = { query = "q" } } } },
			{ text = "kept", calls = { { name = "note_write", args = { name = "n", text = "body" } } } },
		},
	})
	run:turn_end({ turn = 2, text = "", calls = {}, commit = { change = "k", paths = { "a" } } })
	t.equal(run.files["d/commits/2.json"], '{"change":"k","paths":["a"]}')
	run:run_end({})
	t.equal(run.state.answer.ok, true)
	t.equal(run.state.answer.value, "kept")
	t.equal(#run.state.answer.calls, 2)
	t.equal(run.files["d/notes/n.md"], "body")
	t.equal(#run.infers, 2)
	t.equal(run.infers[2].turns[1].results[1].text, "nothing about q")
	t.equal(run.infers[1].instructions, "You keep notes.")
end)

t.case("a remote is normalised as the host does", function()
	t.equal(t.normalize_remote("git@GitHub.com:o/n.git"), "github.com/o/n")
	t.equal(t.normalize_remote("ssh://git@host.example:22/a/b/"), "host.example/a/b")
	t.equal(t.normalize_remote("/srv/r.git"), "/srv/r")
	t.equal(t.normalize_remote(" "), nil)
end)
"#;
    let files = runtime::Files {
        tests: BTreeMap::from([("fakes".to_owned(), tests.to_owned())]),
        ..package()
    };
    let loaded = block_on_io(runtime::load("scribe", files)).unwrap();
    let results =
        block_on_io(loaded.test(tokio_util::sync::CancellationToken::new()));
    assert_eq!(results.len(), 2, "{results:?}");
    for result in results {
        assert!(result.passed, "{result:?}");
    }
}
