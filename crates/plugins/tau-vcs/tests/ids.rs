//! What the tools accept as a change and as a path
//! (`docs/reference/vcs.md`, "Scoping rules"): ids only, and paths
//! inside the workspace, each checked against the rule as written.

use std::{path::Path, sync::Arc};

use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    plugin::Plugin,
    tool::{AgentTool, ToolCtx},
};
use tau_testing::block_on;
use tau_vcs::{Identity, Vcs, VcsPlugin};

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

    fn write(&self, name: &str, content: &str) {
        let path = self.dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        let tool = self.tools.iter().find(|t| t.name() == name).unwrap();
        block_on(tool.call(args, ToolCtx::detached()))
            .map(|output| output.details.unwrap_or(Value::Null))
            .map_err(|err| err.to_string())
    }

    fn ok(&self, name: &str, args: Value) -> Value {
        self.call(name, args.clone())
            .unwrap_or_else(|err| panic!("{name} {args} failed: {err}"))
    }

    /// Every change `vcs_log` lists, newest first.
    fn log(&self) -> Vec<Value> {
        self.ok("vcs_log", json!({ "limit": 100 }))["changes"]
            .as_array()
            .unwrap()
            .clone()
    }
}

/// A repository with `count` committed changes and a working copy.
fn repo_with(count: usize) -> Repo {
    let repo = Repo::new();
    for n in 0..count {
        repo.write("a.txt", &format!("{n}\n"));
        repo.ok("vcs_commit", json!({ "message": format!("change {n}") }));
    }
    repo
}

/// Every id the tools show, cut to the 12 characters they print (or
/// more), names its change again, through `vcs_show` and `vcs_diff`, by
/// change id and by commit id.
#[hegel::test(test_cases = 20)]
fn every_shown_short_id_names_its_change(tc: TestCase) {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(6));
    let repo = repo_with(count);
    let changes = repo.log();
    let change = tc.draw(gs::sampled_from(changes.clone()));
    let by_commit = tc.draw(gs::booleans());
    let field = if by_commit { "commit_id" } else { "change_id" };
    let full = change[field].as_str().unwrap();
    let len =
        tc.draw(gs::integers::<usize>().min_value(12).max_value(full.len()));
    let short = &full[..len];

    let shown = repo.ok("vcs_show", json!({ "change": short }));
    assert_eq!(shown["change"][field], json!(full));
    let diffed = repo.ok("vcs_diff", json!({ "change": short }));
    assert_eq!(diffed["change"][field], json!(full));
    // Whitespace around an id is ignored, as a model might pass it.
    let padded =
        repo.ok("vcs_show", json!({ "change": format!(" {short}\n") }));
    assert_eq!(padded["change"][field], json!(full));
}

/// How the tools must read an argument naming a change: the rule in
/// the reference, written out.
#[derive(Debug, PartialEq)]
enum Reading {
    NotAnId,
    ChangePrefix,
    CommitPrefix,
}

fn reading(rev: &str) -> Reading {
    let rev = rev.trim();
    if rev.is_empty() {
        Reading::NotAnId
    } else if rev.bytes().all(|b| (b'k'..=b'z').contains(&b)) {
        Reading::ChangePrefix
    } else if rev
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Reading::CommitPrefix
    } else {
        Reading::NotAnId
    }
}

/// Anything a model might pass for a change: revsets, names, `@`,
/// mixed-case and mixed-alphabet strings, and ids' neighbours.
#[hegel::composite]
fn rev(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(vec![
            "@".to_owned(),
            "@-".to_owned(),
            "main".to_owned(),
            "trunk()".to_owned(),
            "root()".to_owned(),
            "HEAD".to_owned(),
            "a..b".to_owned(),
            "zzzzzzzzzzzz".to_owned(),
            "000000000000".to_owned(),
            "ABCDEF".to_owned(),
            "   ".to_owned(),
            String::new(),
        ]),
        gs::text()
            .alphabet("klmnopqrstuvwxyz")
            .min_size(1)
            .max_size(14),
        gs::text()
            .alphabet("0123456789abcdef")
            .min_size(1)
            .max_size(14),
        gs::text().alphabet("kz09afAG@-().~ ").max_size(10),
        gs::text().max_size(10),
    ))
}

/// A string is refused as "not an id" exactly when the rule says so;
/// otherwise it is looked up as the kind of prefix the rule names, and
/// either found or reported as not matching.
#[hegel::test(test_cases = 300)]
fn a_change_argument_is_read_as_the_rule_says(tc: TestCase) {
    // One repository for the whole run: reading never writes.
    thread_local! {
        static REPO: Repo = repo_with(2);
    }
    let rev = tc.draw(rev());
    let expected = reading(&rev);
    REPO.with(|repo| {
        let result = repo.call("vcs_show", json!({ "change": rev }));
        match expected {
            Reading::NotAnId => {
                let err = result.expect_err("not an id");
                assert!(
                    err.contains("is not a change id or a commit id"),
                    "{rev:?}: {err}"
                );
            }
            Reading::ChangePrefix => match result {
                Ok(shown) => assert!(
                    shown["change"]["change_id"]
                        .as_str()
                        .unwrap()
                        .starts_with(rev.trim()),
                    "{rev:?} showed {shown}"
                ),
                Err(err) => assert!(
                    err.starts_with("No change matches")
                        || err.starts_with("Change id prefix")
                        || err.contains("is hidden"),
                    "{rev:?}: {err}"
                ),
            },
            Reading::CommitPrefix => match result {
                Ok(shown) => assert!(
                    shown["change"]["commit_id"]
                        .as_str()
                        .unwrap()
                        .starts_with(rev.trim()),
                    "{rev:?} showed {shown}"
                ),
                Err(err) => assert!(
                    err.starts_with("No commit matches")
                        || err.starts_with("Commit id prefix"),
                    "{rev:?}: {err}"
                ),
            },
        }
    });
}

/// Where a path argument leads, by the rule in the reference: relative
/// to the workspace root, or absolute inside it; `.` components
/// dropped; `..` refused.
#[derive(Debug, PartialEq)]
enum PathReading {
    /// The repository path it names (`""` is the root).
    Inside(String),
    Outside,
    NotInside,
}

fn path_reading(root: &Path, input: &str) -> PathReading {
    let path = Path::new(input.trim());
    let relative = if path.is_absolute() {
        match path.strip_prefix(root) {
            Ok(rest) => rest.to_owned(),
            Err(_) => return PathReading::Outside,
        }
    } else {
        path.to_owned()
    };
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::Normal(part) => {
                parts.push(part.to_str().unwrap().to_owned())
            }
            _ => return PathReading::NotInside,
        }
    }
    PathReading::Inside(parts.join("/"))
}

const FILES: [&str; 4] = ["a.txt", "dir/b.txt", "dir/sub/c.txt", "é/d.txt"];

/// A path a model might pass: made of the tree's names, `.` and `..`,
/// relative, absolute inside the workspace, or absolute elsewhere.
#[hegel::composite]
fn path_arg(tc: &TestCase, root: String) -> String {
    let parts: Vec<&str> = tc.draw(
        gs::vecs(gs::sampled_from(vec![
            "a.txt", "dir", "sub", "b.txt", "c.txt", "é", "d.txt", ".", "..",
        ]))
        .max_size(4),
    );
    let relative = parts.join("/");
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => format!("{root}/{relative}"),
        1 => format!("/elsewhere/{relative}"),
        2 => format!("./{relative}"),
        _ => relative,
    }
}

/// `vcs_diff`'s `paths`: a path is refused exactly when the rule refuses
/// it, with the rule's message, and otherwise limits the diff to the
/// files at or under the path it names.
#[hegel::test(test_cases = 300)]
fn a_path_argument_is_read_as_the_rule_says(tc: TestCase) {
    thread_local! {
        static REPO: Repo = {
            let repo = Repo::new();
            for file in FILES {
                repo.write(file, "text\n");
            }
            repo
        };
    }
    REPO.with(|repo| {
        let root = repo.dir.path().to_str().unwrap().to_owned();
        let input = tc.draw(path_arg(root.clone()));
        let result = repo.call("vcs_diff", json!({ "paths": [input] }));
        match path_reading(repo.dir.path(), &input) {
            PathReading::Outside => {
                let err = result.expect_err("outside");
                assert!(
                    err.contains("is outside the repository"),
                    "{input:?}: {err}"
                );
            }
            PathReading::NotInside => {
                let err = result.expect_err("not inside");
                assert!(
                    err.contains("is not a path inside the repository"),
                    "{input:?}: {err}"
                );
            }
            PathReading::Inside(path) => {
                let details =
                    result.unwrap_or_else(|err| panic!("{input:?}: {err}"));
                let listed: Vec<&str> = details["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|file| file["path"].as_str().unwrap())
                    .collect();
                let under: Vec<&str> = FILES
                    .iter()
                    .copied()
                    .filter(|file| {
                        path.is_empty()
                            || *file == path
                            || file.starts_with(&format!("{path}/"))
                    })
                    .collect();
                let mut sorted = under.clone();
                sorted.sort();
                assert_eq!(listed, sorted, "{input:?} as {path:?}");
            }
        }
    });
}

/// The same path, relative and absolute, limits the diff the same way.
#[hegel::test(test_cases = 50)]
fn relative_and_absolute_paths_agree(tc: TestCase) {
    let repo = Repo::new();
    for file in FILES {
        repo.write(file, "text\n");
    }
    let path =
        tc.draw(gs::sampled_from(vec!["a.txt", "dir", "dir/sub", "é", "."]));
    let absolute = repo.dir.path().join(path).to_str().unwrap().to_owned();
    let by_relative = repo.ok("vcs_diff", json!({ "paths": [path] }));
    let by_absolute = repo.ok("vcs_diff", json!({ "paths": [absolute] }));
    let files = |details: &Value| details["files"].clone();
    assert_eq!(files(&by_relative), files(&by_absolute));
}
