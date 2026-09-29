//! The plugin in runs: its tools, what a run starts with, the flush when
//! compaction replaces the transcript, stale marks on edits, and the
//! consolidation pass.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use hegel::{TestCase, generators as gs};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tau_agent::{
    agent::Agent,
    error::{PluginError, ToolError},
    event::RunEvent,
    plugin::{ContextView, Plugin, PluginCtx, PluginRun, Rewrite, RunPlan},
    tool::{ToolCtx, ToolOutput, ToolUpdates, TypedTool, typed},
};
use tau_ai::message::{Message, UserContent};
use tau_memory::{
    Memory,
    MemoryPlugin,
    Scopes,
    index::Bm25,
    memory::Draft,
    note::{By, Link, LinkType, NoteType, Source},
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;

thread_local! {
    /// The test's runtime: the store needs I/O, and the memory tools a
    /// blocking pool.
    static RUNTIME: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.with(|runtime| runtime.block_on(future))
}

fn scopes(dir: &std::path::Path, user: bool) -> Scopes {
    let open =
        |sub: &str| Memory::open(dir.join(sub), Box::new(Bm25::new())).unwrap();
    Scopes::new(open("repo"), user.then(|| open("user")), Arc::new(|| 1_000))
}

fn draft(kind: NoteType, title: &str, body: &str) -> Draft {
    Draft {
        kind,
        title: title.into(),
        description: format!("about {title}"),
        body: body.into(),
        tags: Vec::new(),
        links: Vec::new(),
        id: None,
        supersedes: None,
        source: Source::new(By::User),
    }
}

fn tool_ctx() -> ToolCtx {
    ToolCtx::new(
        tokio_util::sync::CancellationToken::new(),
        ToolUpdates::detached(),
        tau_agent::tool::RunId("run".into()),
    )
}

fn text_of(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            tau_ai::message::InputBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}

/// What the model's first request carried before the task: the context
/// the plugins put in.
fn first_user_text(requests: &[tau_testing::scripted::Request]) -> String {
    match &requests[0].transcript[0] {
        Message::User(user) => match &user.content {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    tau_ai::message::InputBlock::Text(t) => {
                        Some(t.text.clone())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        },
        other => panic!("{other:?}"),
    }
}

/// A note written through the tools reads back whole, and is found by
/// its words, in whichever scope it went to.
#[hegel::test(test_cases = 40)]
fn a_note_written_through_the_tools_reads_back(tc: TestCase) {
    let dir = tempfile::tempdir().unwrap();
    let scopes = scopes(dir.path(), true);
    let plugin = MemoryPlugin::new(scopes.clone());
    let tools = plugin.tools();
    let tool =
        |name: &str| tools.iter().find(|t| t.name() == name).unwrap().clone();
    // Notes in both scopes that do not hold the word.
    let others = tc.draw(gs::integers::<usize>().max_value(4));
    for n in 0..others {
        let scope = if n % 2 == 0 {
            &scopes.repo
        } else {
            scopes.user.as_ref().unwrap()
        };
        scope
            .lock()
            .unwrap()
            .write(
                draft(
                    NoteType::Fact,
                    &format!("other note {n}"),
                    "the retry loop reads a header",
                ),
                1,
            )
            .unwrap();
    }
    let word: String = tc.draw(gs::from_regex("[a-z]{6,10}"));
    tc.assume(
        !["other", "note", "about", "retry", "header"].contains(&word.as_str()),
    );
    // Any text but the characters that hide text, which are refused
    // however the rest reads.
    let noise: String = tc.draw(gs::text().max_size(80));
    let noise: String = noise
        .chars()
        .filter(|c| tau_memory::safety::refusal(&c.to_string()).is_none())
        .collect();
    let body = format!("{noise} {word}");
    let scope = tc.draw(gs::sampled_from(vec!["repository", "user"]));
    let kind =
        tc.draw(gs::sampled_from(vec!["fact", "gotcha", "decision", "case"]));
    block_on(async {
        let written = tool("memory_write")
            .call(
                json!({
                    "type": kind,
                    "title": format!("note about {word}"),
                    "description": "a note",
                    "body": body,
                    "scope": scope,
                }),
                tool_ctx(),
            )
            .await;
        // Text that reads as an instruction is refused; nothing else is.
        let Ok(written) = written else {
            assert!(tau_memory::safety::refusal(&body).is_some());
            return;
        };
        let id = written.details.unwrap()["id"].as_str().unwrap().to_owned();
        assert_eq!(id.starts_with("user:"), scope == "user");
        let read = tool("memory_read")
            .call(json!({ "id": id }), tool_ctx())
            .await
            .unwrap();
        let (clean, _) = tau_memory::safety::redact(body.trim_end());
        assert!(
            text_of(&read).contains(clean.as_str()),
            "{}",
            text_of(&read)
        );
        let found = tool("memory_search")
            .call(json!({ "query": word }), tool_ctx())
            .await
            .unwrap();
        assert_eq!(found.details.unwrap()["ids"][0], json!(id));
    });
}

#[test]
fn a_run_starts_with_the_index_and_the_notes_for_its_task() {
    let dir = tempfile::tempdir().unwrap();
    let scopes = scopes(dir.path(), false);
    {
        let mut repo = scopes.repo.lock().unwrap();
        repo.write(
            draft(NoteType::Index, "Index", "- `retry-after`: HTTP dates too"),
            1,
        )
        .unwrap();
        repo.write(
            draft(
                NoteType::Gotcha,
                "Retry after",
                "retry-after can be an HTTP date",
            ),
            1,
        )
        .unwrap();
    }
    let model = ScriptedModel::new().turn(|t| t.text("done"));
    let agent = Agent::new(model.clone()).plugin(MemoryPlugin::new(scopes));
    block_on(async {
        let store = Store::memory().await.unwrap();
        agent
            .run("fix the retry after parsing", &store)
            .await
            .unwrap();
    });
    let text = first_user_text(&model.requests());
    assert!(text.contains("<memory"), "{text}");
    assert!(text.contains("HTTP dates too"), "the index note: {text}");
    assert!(text.contains("`retry-after` [gotcha]"), "the hit: {text}");
    // The tools are offered.
    let tools: Vec<String> = model.requests()[0]
        .settings
        .tools
        .iter()
        .map(|t| t.name.clone())
        .collect();
    for name in [
        "memory_write",
        "memory_search",
        "memory_read",
        "memory_link",
    ] {
        assert!(tools.contains(&name.to_owned()), "{tools:?}");
    }
}

/// Rewrites at every turn end, keeping only the last user message.
struct Compactor;

#[async_trait]
impl Plugin for Compactor {
    fn name(&self) -> &str {
        "compactor"
    }

    async fn start(
        &self,
        _: &mut RunPlan,
        _: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        Ok(Box::new(Compactor))
    }
}

#[async_trait]
impl PluginRun for Compactor {
    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        _: &PluginCtx,
    ) -> Result<Option<Rewrite>, PluginError> {
        let from = view
            .transcript
            .iter()
            .rposition(|m| matches!(m, Message::User(_)))
            .unwrap();
        if from == 0 {
            return Ok(None);
        }
        Ok(Some(Rewrite {
            messages: view.transcript[from..].to_vec(),
            details: json!({}),
        }))
    }
}

/// When compaction replaces the transcript, one request with only the
/// memory tools sees what was dropped, and its writes are carried out.
#[test]
fn compaction_gives_memory_one_request_over_what_it_drops() {
    let dir = tempfile::tempdir().unwrap();
    let scopes = scopes(dir.path(), false);
    let model = ScriptedModel::new()
        .turn(|t| t.tool_call("memory_search", json!({"query": "lanes"})))
        // The flush: the memory request answers with a write.
        .turn(|t| {
            t.tool_call(
                "memory_write",
                json!({
                    "type": "gotcha",
                    "title": "Lanes lose their continuation on drain",
                    "description": "a draining socket drops the delta chain",
                    "body": "Seen in the run: resend in full after a drain."
                }),
            )
        })
        .turn(|t| t.text("done"));
    let agent = Agent::new(model.clone())
        .plugin(Compactor)
        .plugin(MemoryPlugin::new(scopes.clone()));
    let events: Vec<RunEvent> = block_on(async {
        let store = Store::memory().await.unwrap();
        let mut run = agent.start("why do lanes stall", &store);
        run.steer("keep going");
        let events = run.events().collect().await;
        run.outcome().await.unwrap();
        events
    });
    let requests = model.requests();
    assert_eq!(requests.len(), 3);
    // The memory request: only the memory tools, the old conversation as text.
    let flush = &requests[1];
    let names: Vec<&str> = flush
        .settings
        .tools
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "memory_write",
            "memory_search",
            "memory_read",
            "memory_link"
        ]
    );
    assert!(
        flush
            .settings
            .instructions
            .as_deref()
            .unwrap()
            .contains("being compacted")
    );
    let Message::User(user) = &flush.transcript[0] else {
        panic!()
    };
    let UserContent::Text(shown) = &user.content else {
        panic!()
    };
    assert!(
        shown.contains("why do lanes stall")
            && shown.contains("[call memory_search]")
    );
    // The note was written, as the agent's.
    let repo = scopes.repo.lock().unwrap();
    let note = repo
        .notes()
        .get("lanes-lose-their-continuation-on-drain")
        .unwrap();
    assert_eq!(note.source.by, By::Agent);
    assert!(events.iter().any(|e| matches!(e,
        RunEvent::PluginReport { plugin, body, .. }
            if &**plugin == "tau-memory" && body["kind"] == "saved")));
}

#[derive(Debug, Deserialize, JsonSchema)]
struct EditArgs {
    path: String,
}

/// Stands in for the edit tool: changes nothing, takes a path.
struct Edit;

#[async_trait]
impl TypedTool for Edit {
    type Args = EditArgs;
    const NAME: &'static str = "edit";
    const DESCRIPTION: &'static str = "Edit a file.";

    async fn call(
        &self,
        args: EditArgs,
        _: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::text(format!("edited {}", args.path)))
    }
}

#[test]
fn editing_a_file_marks_the_notes_about_it() {
    let dir = tempfile::tempdir().unwrap();
    let scopes = scopes(dir.path(), false);
    {
        let mut repo = scopes.repo.lock().unwrap();
        let mut about =
            draft(NoteType::Fact, "Retry reads the header", "in retry.rs");
        about.links.push(Link {
            to: "src/retry.rs".into(),
            kind: LinkType::About,
            why: None,
        });
        repo.write(about, 1).unwrap();
        repo.write(draft(NoteType::Fact, "Unrelated", "elsewhere"), 1)
            .unwrap();
    }
    let model = ScriptedModel::new()
        .turn(|t| t.tool_call("edit", json!({"path": "src/retry.rs"})))
        .turn(|t| t.text("done"));
    let agent = Agent::new(model)
        .tool(typed(Edit))
        .plugin(MemoryPlugin::new(scopes.clone()));
    block_on(async {
        let store = Store::memory().await.unwrap();
        agent.run("change retry", &store).await.unwrap();
    });
    let repo = scopes.repo.lock().unwrap();
    assert!(
        repo.notes()
            .get("retry-reads-the-header")
            .unwrap()
            .stale
            .is_some()
    );
    assert!(repo.notes().get("unrelated").unwrap().stale.is_none());
}

/// Off by default; on, it runs once the run ended, and its notes are
/// marked inferred.
#[test]
fn consolidation_runs_only_when_turned_on() {
    let dir = tempfile::tempdir().unwrap();
    let write = json!({
        "type": "case",
        "title": "Fixed a flaky lane test",
        "description": "a race on drain; resend in full",
        "body": "Task: flaky lane test. Approach: resend after drain. Outcome: fixed."
    });
    for on in [false, true] {
        let scopes = scopes(&dir.path().join(on.to_string()), false);
        let mut model = ScriptedModel::new().turn(|t| t.text("fixed"));
        if on {
            let write = write.clone();
            model = model.turn(move |t| t.tool_call("memory_write", write));
        }
        let agent = Agent::new(model.clone())
            .plugin(MemoryPlugin::new(scopes.clone()).consolidate(on));
        block_on(async {
            let store = Store::memory().await.unwrap();
            agent.run("fix the flaky lane test", &store).await.unwrap();
        });
        assert_eq!(model.requests().len(), if on { 2 } else { 1 });
        let repo = scopes.repo.lock().unwrap();
        let note = repo.notes().get("fixed-a-flaky-lane-test");
        assert_eq!(note.is_some(), on);
        if let Some(note) = note {
            assert_eq!(note.source.by, By::Inferred);
        }
    }
}

#[test]
fn an_unknown_type_is_refused_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let plugin = MemoryPlugin::new(scopes(dir.path(), false));
    let tools = plugin.tools();
    let write = tools.iter().find(|t| t.name() == "memory_write").unwrap();
    let error = block_on(write.call(
        json!({"type": "todo", "title": "t", "description": "d", "body": "b"}),
        tool_ctx(),
    ))
    .unwrap_err();
    assert!(error.to_string().contains("\"todo\" is not a note type"));
}
