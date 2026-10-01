//! The figures: a run's are read correctly from its events and outcome,
//! the summaries are the plain means and rates of their trials, in any
//! order, and the call classifiers find what they claim.

use std::collections::BTreeMap;

use futures_util::StreamExt;
use hegel::{
    TestCase,
    generators as gs,
    generators::{Generator as _, PrintableGenerator},
};
use regex::Regex;
use serde_json::json;
use tau_agent::{agent::Agent, event::RunEvent};
use tau_ai::message::{Message, UserContent, UserMessage};
use tau_memory_e2e::{
    arm::{Arm, CHUNK_CHARS, chunks},
    metrics::{
        Call,
        Meter,
        RunMetrics,
        Summary,
        Trial,
        memory_calls,
        stale_used,
        summarize,
    },
    scenario::Variant,
};
use tau_store::Store;
use tau_testing::scripted::ScriptedModel;
use tau_tools::{path::Root, plugin::CodingTools};

thread_local! {
    static RUNTIME: tokio::runtime::Runtime =
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    RUNTIME.with(|runtime| runtime.block_on(future))
}

/// Turns of tool calls then a last answer: a run's figures are its
/// responses, its calls, and the usage and cost the model reported.
#[hegel::test(test_cases = 30)]
fn a_runs_figures_are_read_from_its_events(tc: TestCase) {
    // A turn with no call ends the run, so every turn but the last has
    // one.
    let calls_per_turn: Vec<usize> = tc.draw(
        gs::vecs(gs::integers::<usize>().min_value(1).max_value(3)).max_size(4),
    );
    let costs: Vec<u32> = (0..=calls_per_turn.len())
        .map(|_| tc.draw(gs::integers::<u32>().max_value(1_000)))
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let mut model = ScriptedModel::new();
    for (turn, calls) in calls_per_turn.iter().enumerate() {
        let (calls, cost) = (*calls, f64::from(costs[turn]) / 1e4);
        model = model.turn(move |mut t| {
            for n in 0..calls {
                // A call that fails and one that works, alternately.
                let path = if n % 2 == 0 { "." } else { "missing" };
                t = t.tool_call("ls", json!({ "path": path }));
            }
            t.cost(cost)
        });
    }
    let last = f64::from(*costs.last().unwrap()) / 1e4;
    model = model.turn(move |t| t.text("done").cost(last));
    let agent = Agent::new(model.clone())
        .model("scripted")
        .plugin(CodingTools::new(Root::new(dir.path().to_owned())));
    let (metrics, events) = block_on(async {
        let store = Store::memory().await.unwrap();
        let mut meter = Meter::new();
        let mut run = agent.start("list files", &store);
        let events: Vec<RunEvent> = run.events().collect().await;
        for event in &events {
            meter.observe(event);
        }
        let outcome = run.outcome().await;
        (
            meter.finish(&outcome, std::time::Duration::from_millis(1234)),
            events,
        )
    });
    let calls: usize = calls_per_turn.iter().sum();
    assert_eq!(metrics.turns as usize, calls_per_turn.len() + 1);
    assert_eq!(metrics.tool_calls as usize, calls);
    let failing: usize = calls_per_turn.iter().map(|n| n.div_ceil(2)).sum();
    let failing = calls - failing;
    assert_eq!(metrics.failed_calls as usize, failing);
    let cost: f64 = costs.iter().map(|c| f64::from(*c) / 1e4).sum();
    assert!((metrics.cost_usd - cost).abs() < 1e-9, "{metrics:?}");
    // Tokens are the turns' usage summed.
    let (mut input, mut output, mut cached) = (0, 0, 0);
    for event in &events {
        if let RunEvent::TurnEnd { usage, .. } = event {
            input += usage.input;
            output += usage.output;
            cached += usage.cache_read;
        }
    }
    assert_eq!(
        (
            metrics.input_tokens,
            metrics.output_tokens,
            metrics.cached_tokens
        ),
        (input, output, cached)
    );
    assert_eq!(metrics.wall_ms, 1234);
    assert_eq!(metrics.stop, "stop");
    model.assert_exhausted();
}

fn call(tool: &str, args: serde_json::Value) -> Call {
    Call {
        tool: tool.into(),
        args,
        is_error: false,
        parent: None,
    }
}

/// Calls a tool made through the loop (a codemode script's) count apart
/// from the model's own, failed ones included, and are still listed.
#[hegel::test(test_cases = 100)]
fn nested_calls_count_apart(tc: TestCase) {
    use std::sync::Arc;

    use tau_agent::tool::{RunId, ToolOutput};
    // (nested?, failed?)
    let calls: Vec<(bool, bool)> =
        tc.draw(gs::vecs(hegel::tuples!(gs::booleans(), gs::booleans())));
    let run = RunId("r".into());
    let mut meter = Meter::new();
    let mut top = 0;
    for (n, (nested, failed)) in calls.iter().enumerate() {
        let (call_id, parent) = if *nested {
            (format!("c{top}/{n}"), Some(format!("c{top}")))
        } else {
            top = n;
            (format!("c{n}"), None)
        };
        meter.observe(&RunEvent::ToolStart {
            run: run.clone(),
            call_id: call_id.clone(),
            tool: "read".into(),
            args: json!({}),
            parent: parent.clone(),
        });
        meter.observe(&RunEvent::ToolEnd {
            run: run.clone(),
            call_id,
            output: Arc::new(ToolOutput::text("")),
            is_error: *failed,
            parent,
        });
    }
    let outcome =
        Err(tau_agent::agent::AgentError::OutputSchema("none".into()));
    let metrics = meter.finish(&outcome, std::time::Duration::ZERO);
    let count = |nested: bool, failed: bool| {
        calls
            .iter()
            .filter(|(n, f)| *n == nested && (!failed || *f))
            .count() as u32
    };
    assert_eq!(metrics.tool_calls, count(false, false));
    assert_eq!(metrics.failed_calls, count(false, true));
    assert_eq!(metrics.nested_calls, count(true, false));
    assert_eq!(meter.calls().len(), calls.len());
}

#[test]
fn memory_reads_are_the_memory_tools_and_memory_md() {
    let calls = [
        call("memory_search", json!({"query": "tests"})),
        call("memory_read", json!({"id": "tests"})),
        call("memory_write", json!({"title": "t"})),
        call("read", json!({"path": "MEMORY.md"})),
        call("bash", json!({"command": "cat MEMORY.md"})),
        call("read", json!({"path": "README.md"})),
        call("bash", json!({"command": "ls"})),
    ];
    assert_eq!(memory_calls(&calls), 4);
    assert_eq!(memory_calls(&calls[5..]), 0);
}

#[test]
fn the_old_fact_counts_when_acted_on_not_when_written_down() {
    let stale = Regex::new(r"\bHARBOR_MODE\b").unwrap();
    let acted = [
        call("bash", json!({"command": "HARBOR_MODE=test ./test.sh"})),
        call("read", json!({"path": "HARBOR_MODE"})),
        call(
            "edit",
            json!({"path": "ci/env.sh", "newText": "HARBOR_MODE"}),
        ),
        call("write", json!({"path": "x.sh", "content": "HARBOR_MODE=1"})),
    ];
    for call in &acted {
        assert!(stale_used(std::slice::from_ref(call), &stale), "{call:?}");
    }
    let noted = [
        call(
            "write",
            json!({"path": "MEMORY.md", "content": "HARBOR_MODE is gone"}),
        ),
        call("memory_write", json!({"body": "HARBOR_MODE was renamed"})),
        call("bash", json!({"command": "HARBOR_ENV=test ./test.sh"})),
        call("grep", json!({"pattern": "HARBOR_MODE"})),
    ];
    assert!(!stale_used(&noted, &stale));
}

/// A transcript's chunks each fit the limit and, between them, hold
/// every message.
#[hegel::test(test_cases = 50)]
fn transcript_chunks_fit_and_keep_every_message(tc: TestCase) {
    let texts: Vec<String> = tc
        .draw(gs::vecs(gs::from_regex("[a-z][a-z ]{0,600}[a-z]")).max_size(12));
    let transcript: Vec<Message> = texts
        .iter()
        .map(|text| {
            Message::User(UserMessage {
                content: UserContent::Text(text.clone()),
                timestamp: 0,
            })
        })
        .collect();
    let chunks = chunks(&transcript);
    for chunk in &chunks {
        assert!(chunk.chars().count() <= CHUNK_CHARS, "{}", chunk.len());
    }
    for text in &texts {
        assert!(chunks.iter().any(|chunk| chunk.contains(text.as_str())));
    }
}

fn run_metrics(tc: &TestCase) -> RunMetrics {
    RunMetrics {
        success: Some(tc.draw(gs::booleans())),
        turns: tc.draw(gs::integers::<u32>().max_value(50)),
        tool_calls: tc.draw(gs::integers::<u32>().max_value(50)),
        failed_calls: tc.draw(gs::integers::<u32>().max_value(5)),
        nested_calls: tc.draw(gs::integers::<u32>().max_value(50)),
        input_tokens: tc.draw(gs::integers::<u64>().max_value(1_000_000)),
        output_tokens: tc.draw(gs::integers::<u64>().max_value(100_000)),
        cached_tokens: tc.draw(gs::integers::<u64>().max_value(1_000_000)),
        cost_usd: f64::from(tc.draw(gs::integers::<u32>().max_value(100_000)))
            / 1e4,
        wall_ms: tc.draw(gs::integers::<u64>().max_value(600_000)),
        stop: "stop".into(),
    }
}

fn trial() -> impl PrintableGenerator<Trial> {
    // Trial is this crate's own type, so drawn values print through Debug.
    trial_unprinted().print_as_debug()
}

#[hegel::composite]
fn trial_unprinted(tc: &TestCase) -> Trial {
    let variant =
        tc.draw(gs::sampled_from(Variant::ALL.to_vec()).print_as_debug());
    Trial {
        scenario: "s".into(),
        variant,
        arm: tc.draw(gs::sampled_from(Arm::ALL.to_vec()).print_as_debug()),
        trial: 0,
        first: run_metrics(tc),
        second: run_metrics(tc),
        memory_saved: tc.draw(gs::booleans()),
        memory_given: tc.draw(gs::booleans()),
        memory_calls: tc.draw(gs::integers::<u32>().max_value(3)),
        stale_used: (variant == Variant::Changed)
            .then(|| tc.draw(gs::booleans())),
        calls: Vec::new(),
    }
}

/// The summaries by a second, plainer road: group, then count and add.
fn reference(trials: &[Trial]) -> BTreeMap<(Arm, Variant), Summary> {
    let mut groups: BTreeMap<(Arm, Variant), Vec<&Trial>> = BTreeMap::new();
    for trial in trials {
        groups
            .entry((trial.arm, trial.variant))
            .or_default()
            .push(trial);
    }
    let mut out = BTreeMap::new();
    for ((arm, variant), group) in groups {
        let n = group.len() as f64;
        let mut s = Summary {
            arm,
            variant,
            trials: group.len(),
            success_rate: 0.0,
            first_success_rate: 0.0,
            tool_calls: 0.0,
            turns: 0.0,
            input_tokens: 0.0,
            output_tokens: 0.0,
            cached_tokens: 0.0,
            cost_usd: 0.0,
            wall_s: 0.0,
            trial_cost_usd: 0.0,
            stale_rate: None,
            read_memory_rate: 0.0,
        };
        let (mut stale, mut changed) = (0.0, 0.0);
        for t in &group {
            s.success_rate +=
                f64::from(u8::from(t.second.success == Some(true)));
            s.first_success_rate +=
                f64::from(u8::from(t.first.success == Some(true)));
            s.tool_calls += f64::from(t.second.tool_calls);
            s.turns += f64::from(t.second.turns);
            s.input_tokens += t.second.input_tokens as f64;
            s.output_tokens += t.second.output_tokens as f64;
            s.cached_tokens += t.second.cached_tokens as f64;
            s.cost_usd += t.second.cost_usd;
            s.wall_s += t.second.wall_ms as f64 / 1000.0;
            s.trial_cost_usd += t.first.cost_usd + t.second.cost_usd;
            s.read_memory_rate +=
                f64::from(u8::from(t.memory_given || t.memory_calls > 0));
            if let Some(used) = t.stale_used {
                changed += 1.0;
                stale += f64::from(u8::from(used));
            }
        }
        for field in [
            &mut s.success_rate,
            &mut s.first_success_rate,
            &mut s.tool_calls,
            &mut s.turns,
            &mut s.input_tokens,
            &mut s.output_tokens,
            &mut s.cached_tokens,
            &mut s.cost_usd,
            &mut s.wall_s,
            &mut s.trial_cost_usd,
            &mut s.read_memory_rate,
        ] {
            *field /= n;
        }
        s.stale_rate = (changed > 0.0).then_some(stale / changed);
        out.insert((arm, variant), s);
    }
    out
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0)
}

fn same(a: &Summary, b: &Summary) -> bool {
    let pairs = [
        (a.success_rate, b.success_rate),
        (a.first_success_rate, b.first_success_rate),
        (a.tool_calls, b.tool_calls),
        (a.turns, b.turns),
        (a.input_tokens, b.input_tokens),
        (a.output_tokens, b.output_tokens),
        (a.cached_tokens, b.cached_tokens),
        (a.cost_usd, b.cost_usd),
        (a.wall_s, b.wall_s),
        (a.trial_cost_usd, b.trial_cost_usd),
        (a.read_memory_rate, b.read_memory_rate),
    ];
    a.arm == b.arm
        && a.variant == b.variant
        && a.trials == b.trials
        && pairs.iter().all(|(x, y)| close(*x, *y))
        && match (a.stale_rate, b.stale_rate) {
            (Some(x), Some(y)) => close(x, y),
            (None, None) => true,
            _ => false,
        }
}

#[hegel::test(test_cases = 100)]
fn summaries_are_the_means_of_their_group_in_any_order(tc: TestCase) {
    let trials: Vec<Trial> = tc.draw(gs::vecs(trial()).max_size(20));
    let summaries = summarize(&trials);
    let expected = reference(&trials);
    assert_eq!(summaries.len(), expected.len());
    for summary in &summaries {
        let want = &expected[&(summary.arm, summary.variant)];
        assert!(same(summary, want), "{summary:?}\n{want:?}");
    }
    // In arm order, then by variant.
    let order: Vec<(usize, Variant)> = summaries
        .iter()
        .map(|s| (s.arm.rank(), s.variant))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    // The order of the trials does not matter.
    let mut shuffled = trials.clone();
    let rotate = tc.draw(gs::integers::<usize>().max_value(trials.len()));
    shuffled.rotate_left(rotate.min(trials.len()));
    shuffled.reverse();
    let again = summarize(&shuffled);
    assert!(summaries.iter().zip(&again).all(|(a, b)| same(a, b)));
}
