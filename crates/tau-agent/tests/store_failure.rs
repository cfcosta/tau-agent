//! The loop and the store together (`docs/reference/testing.md`,
//! `tau-store`): a write that fails between a tool batch finishing and
//! the turn being stored leaves no partial turn.
//!
//! These tests need a database file, so a second connection can break
//! it mid-run, and so they run on a normal tokio runtime (see "sqlx in
//! tests" in the testing doc).

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use sqlx::Connection;
use tau_agent::{
    agent::{Agent, AgentError},
    tool::{AgentTool, ToolCtx, ToolOutput},
};
use tau_store::{Entry, Status, Store};
use tau_testing::scripted::ScriptedModel;

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

static CASE: AtomicUsize = AtomicUsize::new(0);

/// A fresh database path per case.
fn database() -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "tau-agent-store-failure-{}-{}",
        std::process::id(),
        CASE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    (dir.join("runs.db"), dir)
}

async fn execute(path: &Path, sql: &'static str) {
    let mut connection = sqlx::SqliteConnection::connect(&format!(
        "sqlite://{}",
        path.display()
    ))
    .await
    .unwrap();
    sqlx::query(sql).execute(&mut connection).await.unwrap();
}

/// Breaks the store when called: the `messages` table goes away, so
/// the turn that ran this tool cannot be stored.
struct Sabotage {
    path: PathBuf,
    schema: Value,
}

#[async_trait]
impl AgentTool for Sabotage {
    fn name(&self) -> &str {
        "sabotage"
    }
    fn description(&self) -> &str {
        "Breaks the store."
    }
    fn parameters(&self) -> &Value {
        &self.schema
    }
    async fn call(&self, _: Value, _: ToolCtx) -> anyhow::Result<ToolOutput> {
        execute(&self.path, "ALTER TABLE messages RENAME TO broken").await;
        Ok(ToolOutput::text("done"))
    }
}

/// Echoes, harmlessly.
struct Echo(Value);

#[async_trait]
impl AgentTool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Echoes."
    }
    fn parameters(&self) -> &Value {
        &self.0
    }
    async fn call(
        &self,
        args: Value,
        _: ToolCtx,
    ) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput::text(args.to_string()))
    }
}

/// A write failure after a tool batch, against a model: a run makes a
/// drawn number of complete turns of drawn width, then a turn whose
/// batch breaks the store. The run fails with a store error, and once
/// the store is repaired its transcript holds exactly the turns before
/// the failed one, whole, and its usage totals only theirs: the failed
/// turn's messages and usage were written together or not at all.
#[hegel::test(test_cases = 20)]
fn a_failed_turn_write_leaves_no_partial_turn(tc: TestCase) {
    let widths: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(1).max_value(3)).max_size(3),
    );
    let echoes_in_failed_turn = tc.draw(gs::integers::<usize>().max_value(2));
    let mut llm = ScriptedModel::new();
    for &width in &widths {
        llm = llm.turn(move |mut t| {
            for i in 0..width {
                t = t.tool_call("echo", json!({"i": i}));
            }
            t.usage(100, 10)
        });
    }
    llm = llm.turn(move |mut t| {
        for i in 0..echoes_in_failed_turn {
            t = t.tool_call("echo", json!({"i": i}));
        }
        t.tool_call("sabotage", json!({})).usage(1_000, 100)
    });
    let (path, dir) = database();
    block_on(async {
        let store = Store::open(&path).await.unwrap();
        let schema = json!({"type": "object"});
        let agent = Agent::new(llm).tool(Echo(schema.clone())).tool(Sabotage {
            path: path.clone(),
            schema,
        });
        let run = agent.start("go", &store);
        let id = run.id();
        let error = run.outcome().await.unwrap_err();
        assert!(matches!(error, AgentError::Store(_)), "{error:?}");

        execute(&path, "ALTER TABLE broken RENAME TO messages").await;
        let entries = store.transcript(&id.0).await.unwrap();
        let roles: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                Entry::Message { role, .. } => role.as_str(),
                Entry::Compaction { .. } => panic!("no compaction here"),
            })
            .collect();
        let mut expected = vec!["user"];
        for &width in &widths {
            expected.push("assistant");
            expected.extend(std::iter::repeat_n("toolResult", width));
        }
        assert_eq!(roles, expected);
        let record = store.run(&id.0).await.unwrap().unwrap();
        assert_eq!(record.output_tokens, 10 * widths.len() as i64);
        // The run could not record its end either.
        assert_eq!(record.status, Status::Running);
    });
    std::fs::remove_dir_all(dir).unwrap();
}
