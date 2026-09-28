//! The store (`tau_store`), checked against a `Vec`-based model over
//! random sequences of run creation, appends (context rewrites and plugin
//! records included) and finishes.
//!
//! These tests use a normal tokio runtime, not tau-testing's paused one:
//! sqlx's SQLite driver waits on its own worker threads, and paused time
//! would jump forward during those waits and fire sqlx's timeouts.

use std::collections::BTreeMap;

use hegel::{TestCase, generators as gs};
use serde_json::json;
use tau_store::{
    AgentCost,
    Entry,
    NewRun,
    RunKind,
    Status,
    Store,
    StoreError,
    TurnUsage,
    WriterStats,
};

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[derive(Debug, Clone)]
struct ModelRun {
    kind: RunKind,
    agent: &'static str,
    workflow: Option<&'static str>,
    own: Vec<Entry>,
    input: i64,
    output: i64,
    cost: f64,
    status: Status,
}

#[derive(Default)]
struct Model {
    runs: BTreeMap<String, ModelRun>,
}

impl Model {
    /// The full chain of `run`: inherited entries, then its own.
    fn chain(&self, run: &str) -> Vec<Entry> {
        let this = &self.runs[run];
        let mut entries = match &this.kind {
            RunKind::Fork { parent, fork_seq } => {
                self.chain_upto(parent, *fork_seq)
            }
            _ => Vec::new(),
        };
        entries.extend(this.own.iter().cloned());
        entries
    }

    /// `run`'s chain, keeping only its own entries with `seq <= cut`.
    fn chain_upto(&self, run: &str, cut: i64) -> Vec<Entry> {
        let this = &self.runs[run];
        let mut entries = match &this.kind {
            RunKind::Fork { parent, fork_seq } => {
                self.chain_upto(parent, *fork_seq)
            }
            _ => Vec::new(),
        };
        let keep = usize::try_from(cut + 1).unwrap_or(0).min(this.own.len());
        entries.extend(this.own[..keep].iter().cloned());
        entries
    }

    /// The transcript: the chain from its latest context
    /// entry onward, without plugin records.
    fn transcript(&self, run: &str) -> Vec<Entry> {
        let chain: Vec<Entry> = self
            .chain(run)
            .into_iter()
            .filter(|e| !matches!(e, Entry::Plugin { .. }))
            .collect();
        let start = chain
            .iter()
            .rposition(|e| matches!(e, Entry::Context { .. }))
            .unwrap_or(0);
        chain[start..].to_vec()
    }

    /// `plugin`'s records along the chain, oldest first.
    fn records(&self, run: &str, plugin: &str) -> Vec<String> {
        self.chain(run)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Plugin { plugin: p, body } if p == plugin => Some(body),
                _ => None,
            })
            .collect()
    }
}

#[hegel::composite]
fn entry(tc: TestCase) -> Entry {
    let text: String = tc.draw(gs::text().max_size(12));
    let plugin = || {
        tc.draw(gs::sampled_from(vec![
            "memory".to_owned(),
            "prune".to_owned(),
        ]))
    };
    match tc.draw(gs::integers::<u8>().max_value(6)) {
        0 => Entry::Context {
            plugin: plugin(),
            body: json!({ "ledger": text }).to_string(),
        },
        1 => Entry::Plugin {
            plugin: plugin(),
            body: json!({ "note": text }).to_string(),
        },
        _ => {
            let role = tc.draw(gs::sampled_from(vec![
                "user".to_owned(),
                "assistant".to_owned(),
                "toolResult".to_owned(),
            ]));
            Entry::Message {
                body: json!({ "role": role.clone(), "text": text, "n": 1.5 })
                    .to_string(),
                role,
            }
        }
    }
}

/// Usage whose cost adds exactly in binary floating point, so the
/// model's sum and SQLite's agree bit for bit.
#[hegel::composite]
fn usage(tc: TestCase) -> TurnUsage {
    TurnUsage {
        input_tokens: tc.draw(gs::integers::<u32>()),
        output_tokens: tc.draw(gs::integers::<u32>()),
        cost_usd: tc.draw(gs::integers::<u32>().max_value(4096)) as f64
            / 1024.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Root,
    Fork,
    Subagent,
    Append,
    AppendUnknown,
    Finish,
}

#[hegel::test(test_cases = 60)]
fn store_matches_model(tc: TestCase) {
    store_matches_model_body(tc)
}

/// [`store_matches_model`] with more cases, for the nightly tier.
#[hegel::test(test_cases = 1000)]
#[ignore = "extended"]
fn store_matches_model_extended(tc: TestCase) {
    store_matches_model_body(tc)
}

fn store_matches_model_body(tc: TestCase) {
    block_on(async {
        let store = Store::memory().await.unwrap();
        let mut model = Model::default();
        let steps = tc.draw(gs::integers::<usize>().min_value(1).max_value(25));
        for step in 0..steps {
            let op = tc.draw(gs::sampled_from(vec![
                Op::Root,
                Op::Fork,
                Op::Subagent,
                Op::Append,
                Op::AppendUnknown,
                Op::Finish,
            ]));
            let ids: Vec<String> = model.runs.keys().cloned().collect();
            let pick = |tc: &TestCase| -> Option<String> {
                (!ids.is_empty())
                    .then(|| tc.draw(gs::sampled_from(ids.clone())))
            };
            tc.note(&format!("step {step}: {op:?}"));
            match op {
                Op::Root | Op::Fork | Op::Subagent => {
                    let kind = match op {
                        Op::Root => RunKind::Root,
                        Op::Fork => {
                            let Some(parent) = pick(&tc) else { continue };
                            let len = model.runs[&parent].own.len() as i64;
                            RunKind::Fork {
                                parent,
                                fork_seq: tc.draw(
                                    gs::integers::<i64>()
                                        .min_value(-1)
                                        .max_value(len + 1),
                                ),
                            }
                        }
                        _ => {
                            let Some(parent) = pick(&tc) else { continue };
                            RunKind::Subagent { parent }
                        }
                    };
                    let id = format!("run_{step}");
                    let agent =
                        tc.draw(gs::sampled_from(vec!["lead", "coder"]));
                    let workflow = tc.draw(gs::sampled_from(vec![
                        None,
                        Some("wf_1"),
                        Some("wf_2"),
                    ]));
                    store
                        .create_run(&NewRun {
                            id: &id,
                            workflow_id: workflow,
                            agent,
                            kind: kind.clone(),
                            model: "gpt-5.5",
                        })
                        .await
                        .unwrap();
                    model.runs.insert(
                        id,
                        ModelRun {
                            kind,
                            agent,
                            workflow,
                            own: Vec::new(),
                            input: 0,
                            output: 0,
                            cost: 0.0,
                            status: Status::Running,
                        },
                    );
                }
                Op::Append => {
                    let Some(run) = pick(&tc) else { continue };
                    let entries: Vec<Entry> =
                        tc.draw(gs::vecs(entry()).max_size(3));
                    let usage = tc.draw(usage());
                    let last =
                        store.append_turn(&run, &entries, usage).await.unwrap();
                    let m = model.runs.get_mut(&run).unwrap();
                    m.own.extend(entries);
                    assert_eq!(
                        last,
                        m.own.len() as i64 - 1,
                        "last seq of {run}"
                    );
                    m.input += i64::from(usage.input_tokens);
                    m.output += i64::from(usage.output_tokens);
                    m.cost += usage.cost_usd;
                }
                Op::AppendUnknown => {
                    let entries: Vec<Entry> =
                        tc.draw(gs::vecs(entry()).min_size(1).max_size(3));
                    let result = store
                        .append_turn("run_missing", &entries, tc.draw(usage()))
                        .await;
                    assert!(
                        matches!(result, Err(StoreError::UnknownRun(_))),
                        "{result:?}"
                    );
                }
                Op::Finish => {
                    let Some(run) = pick(&tc) else { continue };
                    let status = tc.draw(gs::sampled_from(vec![
                        Status::Done,
                        Status::Failed,
                        Status::Cancelled,
                        Status::Limit,
                    ]));
                    let result = tc.draw(gs::optional(gs::text().max_size(8)));
                    store
                        .finish_run(&run, status, result.as_deref(), None)
                        .await
                        .unwrap();
                    model.runs.get_mut(&run).unwrap().status = status;
                }
            }

            // Every read matches the model.
            for (id, m) in &model.runs {
                assert_eq!(
                    store.transcript(id).await.unwrap(),
                    model.transcript(id),
                    "transcript of {id}"
                );
                for plugin in ["memory", "prune"] {
                    assert_eq!(
                        store.records(id, plugin).await.unwrap(),
                        model.records(id, plugin),
                        "{plugin} records of {id}"
                    );
                }
                let record = store.run(id).await.unwrap().expect("stored run");
                assert_eq!(record.kind, m.kind, "{id}");
                assert_eq!(record.status, m.status, "{id}");
                assert_eq!(
                    (record.input_tokens, record.output_tokens),
                    (m.input, m.output),
                    "{id}"
                );
                assert_eq!(record.cost_usd, m.cost, "{id}");
                assert_eq!(record.agent, m.agent);
                assert_eq!(record.workflow_id.as_deref(), m.workflow);
            }
            for workflow in ["wf_1", "wf_2"] {
                let mut expected: BTreeMap<&str, (i64, f64)> = BTreeMap::new();
                for m in
                    model.runs.values().filter(|m| m.workflow == Some(workflow))
                {
                    let e = expected.entry(m.agent).or_default();
                    e.0 += 1;
                    e.1 += m.cost;
                }
                let expected: Vec<AgentCost> = expected
                    .into_iter()
                    .map(|(agent, (runs, usd))| AgentCost {
                        agent: agent.into(),
                        runs,
                        usd,
                    })
                    .collect();
                assert_eq!(
                    store.workflow_cost(workflow).await.unwrap(),
                    expected,
                    "{workflow}"
                );
            }
        }
    });
}

/// A file-backed store keeps its data across reopening, and runs the
/// migrations only once.
#[test]
fn file_store_survives_reopen() {
    block_on(async {
        let dir = std::env::temp_dir()
            .join(format!("tau-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("runs.db");
        let _ = std::fs::remove_file(&path);
        {
            let store = Store::open(&path).await.unwrap();
            store
                .create_run(&NewRun {
                    id: "r",
                    workflow_id: None,
                    agent: "a",
                    kind: RunKind::Root,
                    model: "m",
                })
                .await
                .unwrap();
            store
                .append_turn(
                    "r",
                    &[Entry::Message {
                        role: "user".into(),
                        body: json!({"text": "hi"}).to_string(),
                    }],
                    TurnUsage::default(),
                )
                .await
                .unwrap();
        }
        let store = Store::open(&path).await.unwrap();
        assert_eq!(
            store.transcript("r").await.unwrap(),
            vec![Entry::Message {
                role: "user".into(),
                body: json!({"text": "hi"}).to_string()
            }]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    });
}

/// Unknown runs are reported, not silently ignored.
#[test]
fn unknown_runs_are_errors() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        assert!(matches!(
            store.finish_run("x", Status::Done, None, None).await,
            Err(StoreError::UnknownRun(_))
        ));
        assert_eq!(store.run("x").await.unwrap(), None);
        assert_eq!(store.transcript("x").await.unwrap(), Vec::<Entry>::new());
    });
}

/// Regression from `store_matches_model`: token counts used to be `i64`,
/// and a negative count drove the run total below `i64::MIN`, which
/// SQLite then refused as a REAL. Counts are unsigned now; the largest
/// ones still add up exactly.
#[test]
fn large_token_counts_add_exactly() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        store
            .create_run(&NewRun {
                id: "r",
                workflow_id: None,
                agent: "a",
                kind: RunKind::Root,
                model: "m",
            })
            .await
            .unwrap();
        let usage = TurnUsage {
            input_tokens: u32::MAX,
            output_tokens: u32::MAX,
            cost_usd: 0.0,
        };
        for _ in 0..3 {
            store.append_turn("r", &[], usage).await.unwrap();
        }
        let run = store.run("r").await.unwrap().unwrap();
        assert_eq!(run.input_tokens, 3 * i64::from(u32::MAX));
        assert_eq!(run.output_tokens, 3 * i64::from(u32::MAX));
    });
}

fn new_run(id: &str) -> NewRun<'_> {
    NewRun {
        id,
        workflow_id: None,
        agent: "a",
        kind: RunKind::Root,
        model: "m",
    }
}

/// Every write counts toward the writer stats, and a clone of the store
/// shares them: creating, appending and finishing are three writes. The
/// longest wait is one of the waits, so it never exceeds their total.
#[test]
fn writes_are_counted_across_clones() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        assert_eq!(store.writer_stats(), WriterStats::default());
        let clone = store.clone();
        store.create_run(&new_run("r")).await.unwrap();
        clone
            .append_turn("r", &[], TurnUsage::default())
            .await
            .unwrap();
        store
            .finish_run("r", Status::Done, None, None)
            .await
            .unwrap();
        // Reads are not writes.
        store.transcript("r").await.unwrap();
        let stats = clone.writer_stats();
        assert_eq!(stats.writes, 3);
        assert!(stats.longest <= stats.waited, "{stats:?}");
    });
}

/// A write that finds the database's write lock held (here, by another
/// connection, as another process would) waits for it, and that wait is
/// what the stats report.
#[test]
fn waiting_for_the_write_lock_is_measured() {
    use sqlx::Connection;
    block_on(async {
        let dir = std::env::temp_dir()
            .join(format!("tau-store-wait-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("runs.db");
        let _ = std::fs::remove_file(&path);
        let store = Store::open(&path).await.unwrap();
        store.create_run(&new_run("r")).await.unwrap();

        let mut other = sqlx::SqliteConnection::connect(&format!(
            "sqlite://{}",
            path.display()
        ))
        .await
        .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut other)
            .await
            .unwrap();
        let held = std::time::Duration::from_millis(150);
        let release = tokio::spawn(async move {
            tokio::time::sleep(held).await;
            sqlx::query("COMMIT").execute(&mut other).await.unwrap();
        });
        store
            .append_turn("r", &[], TurnUsage::default())
            .await
            .unwrap();
        release.await.unwrap();

        let stats = store.writer_stats();
        assert_eq!(stats.writes, 2);
        assert!(stats.longest >= held / 2, "{stats:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    });
}

/// History lists root runs and forks, newest first, without sub-agents.
#[test]
fn recent_runs_skip_subagents() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        for (id, kind) in [
            ("a", RunKind::Root),
            ("b", RunKind::Subagent { parent: "a".into() }),
            (
                "c",
                RunKind::Fork {
                    parent: "a".into(),
                    fork_seq: 0,
                },
            ),
        ] {
            store
                .create_run(&NewRun {
                    id,
                    workflow_id: None,
                    agent: "coder",
                    kind,
                    model: "m",
                })
                .await
                .unwrap();
        }
        let runs = store.recent_runs(10).await.unwrap();
        let ids: Vec<&str> = runs.iter().map(|run| run.id.as_str()).collect();
        assert_eq!(ids, ["c", "a"]);
        assert!(!runs[0].created_at.is_empty());
        assert_eq!(store.recent_runs(1).await.unwrap().len(), 1);
    });
}

/// A plugin's own records come back with their `seq`, to fork at.
#[test]
fn plugin_entries_carry_their_seq() {
    block_on(async {
        let store = Store::memory().await.unwrap();
        store
            .create_run(&NewRun {
                id: "r",
                workflow_id: None,
                agent: "coder",
                kind: RunKind::Root,
                model: "m",
            })
            .await
            .unwrap();
        let message = Entry::Message {
            role: "user".into(),
            body: json!({"text": "hi"}).to_string(),
        };
        let record = |n: u32| Entry::Plugin {
            plugin: "workspace".into(),
            body: json!({ "turn": n }).to_string(),
        };
        let other = Entry::Plugin {
            plugin: "memory".into(),
            body: json!({}).to_string(),
        };
        store
            .append_turn(
                "r",
                &[message.clone(), record(1), other, message, record(2)],
                TurnUsage::default(),
            )
            .await
            .unwrap();
        let entries = store.plugin_entries("r", "workspace").await.unwrap();
        assert_eq!(
            entries,
            vec![
                (1, json!({"turn": 1}).to_string()),
                (4, json!({"turn": 2}).to_string()),
            ]
        );
    });
}
