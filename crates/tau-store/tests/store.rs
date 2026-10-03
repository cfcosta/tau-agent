//! The store (`tau_store`), checked against a `Vec`-based model over
//! random sequences of run creation, appends (context rewrites and plugin
//! records included) and finishes.
//!
//! These tests use a normal tokio runtime, not tau-testing's paused one:
//! sqlx's SQLite driver waits on its own worker threads, and paused time
//! would jump forward during those waits and fire sqlx's timeouts.

use std::collections::BTreeMap;

use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use serde_json::json;
use tau_store::{
    AgentCost,
    Entry,
    NewRun,
    PluginCost,
    RunKind,
    Status,
    Store,
    StoreError,
    TurnUsage,
    WriterStats,
};
use tau_testing::block_on_io;

#[derive(Debug, Clone)]
struct ModelRun {
    kind: RunKind,
    agent: &'static str,
    workflow: Option<&'static str>,
    own: Vec<Entry>,
    input: i64,
    output: i64,
    cost: f64,
    turns: i64,
    status: Status,
    model: &'static str,
    result: Option<String>,
    error: Option<String>,
    title: Option<String>,
    /// What each plugin charged: input, output and cost.
    plugins: BTreeMap<&'static str, (i64, i64, f64)>,
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
            RunKind::Fork { parent, fork_seq }
            | RunKind::Subagent {
                parent,
                fork_seq: Some(fork_seq),
            } => self.chain_upto(parent, *fork_seq),
            _ => Vec::new(),
        };
        entries.extend(this.own.iter().cloned());
        entries
    }

    /// `run`'s chain, keeping only its own entries with `seq <= cut`.
    fn chain_upto(&self, run: &str, cut: i64) -> Vec<Entry> {
        let this = &self.runs[run];
        let mut entries = match &this.kind {
            RunKind::Fork { parent, fork_seq }
            | RunKind::Subagent {
                parent,
                fork_seq: Some(fork_seq),
            } => self.chain_upto(parent, *fork_seq),
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

    /// The transcript with the records in place: the chain from its
    /// latest context entry.
    fn timeline(&self, run: &str) -> Vec<Entry> {
        let chain = self.chain(run);
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

    /// `plugin`'s records in `run` itself, with the `seq` each has: its
    /// index among the run's own entries.
    fn plugin_entries(&self, run: &str, plugin: &str) -> Vec<(i64, String)> {
        self.runs[run]
            .own
            .iter()
            .enumerate()
            .filter_map(|(seq, e)| match e {
                Entry::Plugin { plugin: p, body } if p == plugin => {
                    Some((seq as i64, body.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

fn entry() -> impl PrintableGenerator<Entry> {
    // Entry is tau's own type, so its drawn values print through Debug.
    entry_unprinted().print_as_debug()
}

#[hegel::composite]
fn entry_unprinted(tc: &TestCase) -> Entry {
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
fn usage() -> impl PrintableGenerator<TurnUsage> {
    // TurnUsage is tau's own type, so its drawn values print through Debug.
    usage_unprinted().print_as_debug()
}

#[hegel::composite]
fn usage_unprinted(tc: &TestCase) -> TurnUsage {
    TurnUsage {
        input_tokens: tc.draw(gs::integers::<u32>()),
        output_tokens: tc.draw(gs::integers::<u32>()),
        cost_usd: tc.draw(gs::integers::<u32>().max_value(4096)) as f64
            / 1024.0,
        turns: tc.draw(gs::integers::<u32>().max_value(1)),
    }
}

/// The store and the model it must match, driven one operation at a
/// time: create a run (a root, a fork or a sub-agent), append a turn to
/// it or to a run that does not exist, finish it, or reopen it.
struct StoreMachine {
    /// sqlx needs a real runtime, not a paused one; see the module docs.
    runtime: tokio::runtime::Runtime,
    store: Store,
    model: Model,
    /// Runs created so far, for fresh ids.
    created: usize,
}

impl StoreMachine {
    fn new() -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let store = runtime.block_on(Store::memory()).unwrap();
        Self {
            runtime,
            store,
            model: Model::default(),
            created: 0,
        }
    }

    /// A run the model knows, drawn; rejects the step when there is none.
    fn pick(&self, tc: &TestCase) -> String {
        tc.assume(!self.model.runs.is_empty());
        let ids: Vec<String> = self.model.runs.keys().cloned().collect();
        tc.draw(gs::sampled_from(ids))
    }

    fn create(&mut self, tc: &TestCase, kind: RunKind) {
        let id = format!("run_{}", self.created);
        self.created += 1;
        let agent = tc.draw(gs::sampled_from(vec!["lead", "coder"]));
        let workflow =
            tc.draw(gs::sampled_from(vec![None, Some("wf_1"), Some("wf_2")]));
        self.runtime
            .block_on(self.store.create_run(&NewRun {
                id: &id,
                workflow_id: workflow,
                agent,
                kind: kind.clone(),
                model: "gpt-5.5",
                turns: 0,
            }))
            .unwrap();
        self.model.runs.insert(
            id,
            ModelRun {
                kind,
                agent,
                workflow,
                own: Vec::new(),
                input: 0,
                output: 0,
                cost: 0.0,
                turns: 0,
                status: Status::Running,
                model: "gpt-5.5",
                result: None,
                error: None,
                title: None,
                plugins: BTreeMap::new(),
            },
        );
    }
}

#[hegel::state_machine]
impl StoreMachine {
    #[rule]
    fn create_root(&mut self, tc: TestCase) {
        self.create(&tc, RunKind::Root);
    }

    /// A fork at any `seq` of its parent, and a little past either end.
    #[rule]
    fn create_fork(&mut self, tc: TestCase) {
        let parent = self.pick(&tc);
        let len = self.model.runs[&parent].own.len() as i64;
        let fork_seq =
            tc.draw(gs::integers::<i64>().min_value(-1).max_value(len + 1));
        self.create(&tc, RunKind::Fork { parent, fork_seq });
    }

    /// A sub-agent that starts blank, or forks its parent as a
    /// delegate does, anywhere in it.
    #[rule]
    fn create_subagent(&mut self, tc: TestCase) {
        let parent = self.pick(&tc);
        let len = self.model.runs[&parent].own.len() as i64;
        let fork_seq = tc.draw(gs::optional(
            gs::integers::<i64>().min_value(-1).max_value(len + 1),
        ));
        self.create(&tc, RunKind::Subagent { parent, fork_seq });
    }

    /// Appending returns the `seq` of the last entry the run now holds.
    #[rule]
    fn append(&mut self, tc: TestCase) {
        let run = self.pick(&tc);
        let entries: Vec<Entry> = tc.draw(gs::vecs(entry()).max_size(3));
        let usage = tc.draw(usage());
        let last = self
            .runtime
            .block_on(self.store.append_turn(&run, &entries, usage))
            .unwrap();
        let m = self.model.runs.get_mut(&run).unwrap();
        m.own.extend(entries);
        assert_eq!(last, m.own.len() as i64 - 1, "last seq of {run}");
        m.input += i64::from(usage.input_tokens);
        m.output += i64::from(usage.output_tokens);
        m.cost += usage.cost_usd;
        m.turns += i64::from(usage.turns);
    }

    /// What plugins charged goes to the run's totals with the turn, and
    /// to each plugin's own line, adding up across writes.
    #[rule]
    fn append_charged(&mut self, tc: TestCase) {
        let run = self.pick(&tc);
        let entries: Vec<Entry> = tc.draw(gs::vecs(entry()).max_size(2));
        let total = tc.draw(usage());
        let plugins: Vec<(&'static str, TurnUsage)> = tc.draw(
            gs::vecs(hegel::tuples!(
                gs::sampled_from(vec!["tau-goal", "tau-reasoning"]),
                usage(),
            ))
            .max_size(3),
        );
        self.runtime
            .block_on(
                self.store.append_charged(&run, &entries, total, &plugins),
            )
            .unwrap();
        let m = self.model.runs.get_mut(&run).unwrap();
        m.own.extend(entries);
        m.input += i64::from(total.input_tokens);
        m.output += i64::from(total.output_tokens);
        m.cost += total.cost_usd;
        m.turns += i64::from(total.turns);
        for (plugin, usage) in plugins {
            let line = m.plugins.entry(plugin).or_default();
            line.0 += i64::from(usage.input_tokens);
            line.1 += i64::from(usage.output_tokens);
            line.2 += usage.cost_usd;
        }
    }

    #[rule]
    fn append_to_unknown_run(&mut self, tc: TestCase) {
        let entries: Vec<Entry> =
            tc.draw(gs::vecs(entry()).min_size(1).max_size(3));
        let usage = tc.draw(usage());
        let result = self.runtime.block_on(self.store.append_turn(
            "run_missing",
            &entries,
            usage,
        ));
        assert!(
            matches!(result, Err(StoreError::UnknownRun(_))),
            "{result:?}"
        );
    }

    #[rule]
    fn finish(&mut self, tc: TestCase) {
        let run = self.pick(&tc);
        let status = tc.draw(
            gs::sampled_from(vec![
                Status::Done,
                Status::Failed,
                Status::Cancelled,
                Status::Limit,
            ])
            .print_as_debug(),
        );
        let result = tc.draw(gs::optional(gs::text().max_size(8)));
        let error = tc.draw(gs::optional(gs::text().max_size(8)));
        self.runtime
            .block_on(self.store.finish_run(
                &run,
                status,
                result.as_deref(),
                error.as_deref(),
            ))
            .unwrap();
        let m = self.model.runs.get_mut(&run).unwrap();
        m.status = status;
        m.result = result;
        m.error = error;
    }

    /// A finished run reopens on a drawn model, without its result or
    /// error; a running one refuses.
    #[rule]
    fn reopen(&mut self, tc: TestCase) {
        let run = self.pick(&tc);
        let on = tc.draw(gs::sampled_from(vec!["gpt-5.5", "gpt-6-sol"]));
        let result = self.runtime.block_on(self.store.reopen_run(&run, on));
        let m = self.model.runs.get_mut(&run).unwrap();
        if m.status == Status::Running {
            assert!(
                matches!(result, Err(StoreError::StillRunning(_))),
                "{result:?}"
            );
        } else {
            let record = result.unwrap();
            assert_eq!(record.status, Status::Running);
            assert_eq!(record.result, None);
            assert_eq!(record.error, None);
            assert_eq!(record.model, on, "it goes on on the new model");
            m.status = Status::Running;
            m.model = on;
            m.result = None;
            m.error = None;
        }
    }

    /// A process opening the store marks the runs left running as
    /// interrupted, and no others; it says which. An interrupted run
    /// reopens like any finished one.
    #[rule]
    fn interrupt(&mut self, _tc: TestCase) {
        let ids = self
            .runtime
            .block_on(self.store.interrupt_running())
            .unwrap();
        let mut running: Vec<String> = Vec::new();
        for (id, m) in self.model.runs.iter_mut() {
            if m.status == Status::Running {
                m.status = Status::Interrupted;
                running.push(id.clone());
            }
        }
        running.sort();
        assert_eq!(ids, running);
    }

    /// Naming a run replaces its title and leaves everything else;
    /// naming one that does not exist fails.
    #[rule]
    fn name(&mut self, tc: TestCase) {
        let title = tc.draw(gs::text().min_size(1).max_size(8));
        if tc.draw(gs::booleans()) {
            let result = self
                .runtime
                .block_on(self.store.set_title("run_missing", &title));
            assert!(
                matches!(result, Err(StoreError::UnknownRun(_))),
                "{result:?}"
            );
            return;
        }
        let run = self.pick(&tc);
        self.runtime
            .block_on(self.store.set_title(&run, &title))
            .unwrap();
        self.model.runs.get_mut(&run).unwrap().title = Some(title);
    }

    /// Every read matches the model, after every operation.
    #[invariant(always_run)]
    fn reads_match_the_model(&self, _tc: TestCase) {
        self.runtime.block_on(self.check_reads());
    }
}

impl StoreMachine {
    async fn check_reads(&self) {
        let (store, model) = (&self.store, &self.model);
        for (id, m) in &model.runs {
            assert_eq!(
                store.transcript(id).await.unwrap(),
                model.transcript(id),
                "transcript of {id}"
            );
            assert_eq!(
                store.timeline(id).await.unwrap(),
                model.timeline(id),
                "timeline of {id}"
            );
            for plugin in ["memory", "prune"] {
                assert_eq!(
                    store.records(id, plugin).await.unwrap(),
                    model.records(id, plugin),
                    "{plugin} records of {id}"
                );
                // Everywhere, a run's own entries are its part.
                let own: Vec<String> = store
                    .plugin_entries_everywhere(plugin)
                    .await
                    .unwrap()
                    .into_iter()
                    .filter(|(run, _)| run == id)
                    .map(|(_, body)| body)
                    .collect();
                let entries: Vec<String> = model
                    .plugin_entries(id, plugin)
                    .into_iter()
                    .map(|(_, body)| body)
                    .collect();
                assert_eq!(own, entries, "{plugin} everywhere, in {id}");
                assert_eq!(
                    store.plugin_entries(id, plugin).await.unwrap(),
                    model.plugin_entries(id, plugin),
                    "{plugin} entries of {id}"
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
            assert_eq!(record.turns, m.turns, "{id}");
            assert_eq!(record.agent, m.agent);
            assert_eq!(record.model, m.model, "{id}");
            assert_eq!(record.workflow_id.as_deref(), m.workflow);
            assert_eq!(record.result, m.result, "result of {id}");
            assert_eq!(record.error, m.error, "error of {id}");
            assert_eq!(record.title, m.title, "title of {id}");
            let costs: Vec<PluginCost> = m
                .plugins
                .iter()
                .map(|(plugin, (input, output, usd))| PluginCost {
                    plugin: (*plugin).into(),
                    input_tokens: *input,
                    output_tokens: *output,
                    cost_usd: *usd,
                })
                .collect();
            assert_eq!(
                store.plugin_costs(id).await.unwrap(),
                costs,
                "plugin costs of {id}"
            );
            let first = m.own.iter().find_map(|entry| match entry {
                Entry::Message { role, body } if role == "user" => {
                    Some(body.clone())
                }
                _ => None,
            });
            assert_eq!(
                store.first_prompt(id).await.unwrap(),
                first,
                "first prompt of {id}"
            );
        }
        // Every run started after the epoch, so all of them count.
        let mut spend: BTreeMap<&str, f64> = BTreeMap::new();
        for m in model.runs.values() {
            for (plugin, (_, _, usd)) in &m.plugins {
                *spend.entry(plugin).or_default() += usd;
            }
        }
        let spend: Vec<(String, f64)> = spend
            .into_iter()
            .map(|(plugin, usd)| (plugin.into(), usd))
            .collect();
        assert_eq!(store.plugin_spend("1970").await.unwrap(), spend);
        assert_eq!(
            store.plugin_spend("9999").await.unwrap(),
            Vec::new(),
            "no run started after"
        );
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
}

/// The store matches a `Vec`-based model over random sequences of
/// operations: every read, after every operation.
#[hegel::test(test_cases = 60)]
fn store_matches_model(tc: TestCase) {
    hegel::stateful::machine(StoreMachine::new())
        .steps(25)
        .run(tc);
}

/// [`store_matches_model`] with more cases, for the nightly tier.
#[hegel::test(profile = "nightly_slow")]
#[ignore = "nightly"]
fn store_matches_model_nightly(tc: TestCase) {
    hegel::stateful::machine(StoreMachine::new())
        .steps(25)
        .run(tc);
}

/// A file-backed store keeps its data across reopening, and runs the
/// migrations only once.
#[test]
fn file_store_survives_reopen() {
    block_on_io(async {
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
                    turns: 0,
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
    block_on_io(async {
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
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        store
            .create_run(&NewRun {
                id: "r",
                workflow_id: None,
                agent: "a",
                kind: RunKind::Root,
                model: "m",
                turns: 0,
            })
            .await
            .unwrap();
        let usage = TurnUsage {
            input_tokens: u32::MAX,
            output_tokens: u32::MAX,
            cost_usd: 0.0,
            turns: 0,
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
        turns: 0,
    }
}

/// Every write counts toward the writer stats, and a clone of the store
/// shares them: creating, appending and finishing are three writes. The
/// longest wait is one of the waits, so it never exceeds their total.
#[test]
fn writes_are_counted_across_clones() {
    block_on_io(async {
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
    block_on_io(async {
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

/// A run's sub-agents come back by their parent, oldest first, whatever
/// became of them; a fork is not one.
#[test]
fn subagents_are_found_by_their_parent() {
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        for (id, kind) in [
            ("a", RunKind::Root),
            (
                "b",
                RunKind::Subagent {
                    parent: "a".into(),
                    fork_seq: None,
                },
            ),
            (
                "c",
                RunKind::Subagent {
                    parent: "a".into(),
                    fork_seq: None,
                },
            ),
            (
                "d",
                RunKind::Subagent {
                    parent: "c".into(),
                    fork_seq: None,
                },
            ),
            (
                "e",
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
                    turns: 0,
                })
                .await
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        store
            .finish_run("b", Status::Failed, None, Some("broke"))
            .await
            .unwrap();
        let found = store.subagents("a").await.unwrap();
        let ids: Vec<&str> = found.iter().map(|run| run.id.as_str()).collect();
        assert_eq!(ids, ["b", "c"]);
        assert_eq!(found[0].status, Status::Failed);
        assert!(store.subagents("nobody").await.unwrap().is_empty());
    });
}

/// History lists root runs and forks, newest first, without sub-agents.
#[test]
fn recent_runs_skip_subagents() {
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        for (id, kind) in [
            ("a", RunKind::Root),
            (
                "b",
                RunKind::Subagent {
                    parent: "a".into(),
                    fork_seq: None,
                },
            ),
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
                    turns: 0,
                })
                .await
                .unwrap();
        }
        let runs = store.recent_runs(10).await.unwrap();
        let ids: Vec<&str> = runs.iter().map(|run| run.id.as_str()).collect();
        assert_eq!(ids, ["c", "a"]);
        assert!(!runs[0].created_at.is_empty());
        assert_eq!(store.recent_runs(1).await.unwrap().len(), 1);
        // Activity in a run brings it back to the top.
        std::thread::sleep(std::time::Duration::from_millis(5));
        store
            .append_turn("a", &[], TurnUsage::default())
            .await
            .unwrap();
        let runs = store.recent_runs(10).await.unwrap();
        assert_eq!(runs[0].id, "a");
    });
}

/// A plugin's own records come back with their `seq`, to fork at.
#[test]
fn plugin_entries_carry_their_seq() {
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        store
            .create_run(&NewRun {
                id: "r",
                workflow_id: None,
                agent: "coder",
                kind: RunKind::Root,
                model: "m",
                turns: 0,
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

/// A typed query reads, and only reads.
#[test]
fn typed_queries_only_read() {
    block_on_io(async {
        let store = Store::memory().await.unwrap();
        for id in ["a", "b", "c"] {
            store.create_run(&new_run(id)).await.unwrap();
        }
        let table = store
            .query("select id, agent, result from runs order by id;", 2)
            .await
            .unwrap();
        assert_eq!(table.columns, ["id", "agent", "result"]);
        assert_eq!(
            table.rows,
            [["a", "a", "NULL"], ["b", "a", "NULL"]]
                .map(|row| row.map(String::from))
        );
        assert!(table.truncated);
        let counted = store
            .query("select count(*) as n, 1.5 as x from runs", 10)
            .await
            .unwrap();
        assert_eq!(counted.rows, [["3", "1.5"].map(String::from)]);
        assert!(!counted.truncated);

        // Writing is refused, and the store still writes after.
        assert!(store.query("delete from runs", 10).await.is_err());
        assert!(store.query("select 1; delete from runs", 10).await.is_err());
        assert!(store.query("not sql", 10).await.is_err());
        store.create_run(&new_run("d")).await.unwrap();
        let after = store.query("select count(*) from runs", 10).await.unwrap();
        assert_eq!(after.rows, [["4"].map(String::from)]);
    });
}
