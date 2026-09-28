//! A scripted session for running the interface without an agent: the
//! `retry-after` run from the design mockups, as the run events a real
//! run would stream, plus the plugin updates no event carries yet.

use std::{sync::Arc, time::Duration};

use serde_json::{Value, json};
use tau_agent::{
    event::{RunEvent, StopReason},
    tool::{RunId, ToolOutput},
};
use tau_ai::message::{Usage, UsageCost};

use crate::view::{
    ContextWindow,
    Limits,
    NoteBody,
    PlanField,
    PluginNote,
    PluginStatus,
    Proposal,
    Pruned,
    RunStatus,
    RunUpdate,
    RunView,
    Tone,
    ToolState,
};

/// One scripted update, and how long to wait before it.
pub type Step = (Duration, RunUpdate);

pub fn run_id() -> RunId {
    RunId(Arc::from("retry-after"))
}

/// The run as it looks before its first event.
pub fn retry_after() -> RunView {
    let mut view = RunView::new(run_id(), "retry-after", "coder", "gpt-5.5");
    view.limits = Limits {
        max_turns: Some(20),
        max_tokens: Some(1_000_000),
        max_usd: Some(2.0),
        timeout: Some(Duration::from_secs(600)),
        elapsed: Duration::ZERO,
    };
    view.context = ContextWindow {
        used: 0,
        window: Some(272_000),
        trigger: Some(0.6),
        before: None,
    };
    view.plugins = plugins(["waiting", "waiting", "watching edits", "idle"]);
    view
}

/// Finished runs for the run list.
pub fn history() -> Vec<RunView> {
    [
        ("rotation-jitter", StopReason::Stop, 9, 212_000, 0.231),
        ("plugin-docs", StopReason::Stop, 14, 388_000, 0.42),
        ("mutants-triage", StopReason::Stop, 20, 497_000, 1.07),
        (
            "lane-audit",
            StopReason::Limit(tau_agent::event::LimitKind::Usd),
            17,
            462_000,
            2.0,
        ),
    ]
    .into_iter()
    .map(|(title, stop, turns, tokens, cost)| {
        let mut view =
            RunView::new(RunId(Arc::from(title)), title, "coder", "gpt-5.5");
        view.turn = turns;
        view.usage.tokens = tokens;
        view.usage.cost = cost;
        view.push_user(format!("(stored transcript of {title})"));
        view.status = RunStatus::Finished(stop.clone());
        view.items.push(crate::view::Item::Stop {
            stop,
            turns,
            tokens,
            cost,
            plugin_cost: 0.0,
        });
        view
    })
    .collect()
}

fn plugins(states: [&str; 4]) -> Vec<PluginStatus> {
    let names = [
        "tau-reasoning",
        "tau-memory",
        "tau-constitution",
        "tau-fast-compaction",
    ];
    names
        .into_iter()
        .zip(states)
        .map(|(name, state)| PluginStatus {
            name: name.into(),
            state: state.into(),
            tone: Tone::Quiet,
        })
        .collect()
}

/// Builds the script step by step.
struct Script {
    steps: Vec<Step>,
    run: RunId,
    turn: u32,
    elapsed: Duration,
}

impl Script {
    fn new() -> Self {
        Self {
            steps: Vec::new(),
            run: run_id(),
            turn: 0,
            elapsed: Duration::ZERO,
        }
    }

    fn at(&mut self, millis: u64, update: impl Into<RunUpdate>) {
        let wait = Duration::from_millis(millis);
        self.elapsed += wait;
        self.steps.push((wait, update.into()));
    }

    fn event(&mut self, millis: u64, event: RunEvent) {
        self.at(millis, event);
    }

    fn turn(&mut self) {
        self.turn += 1;
        let turn = self.turn;
        self.event(
            250,
            RunEvent::TurnStart {
                run: self.run.clone(),
                turn,
            },
        );
    }

    fn end_turn(&mut self, input: u64, output: u64, cost: f64) {
        let turn = self.turn;
        self.event(
            150,
            RunEvent::TurnEnd {
                run: self.run.clone(),
                turn,
                usage: Usage {
                    input,
                    output,
                    total_tokens: input + output,
                    cost: UsageCost {
                        total: cost,
                        ..UsageCost::default()
                    },
                    ..Usage::default()
                },
            },
        );
        // Scripted time runs faster than the clock the limits show.
        let elapsed = self.elapsed * 6;
        self.at(
            0,
            RunUpdate::Limits(Limits {
                elapsed,
                ..retry_after().limits
            }),
        );
    }

    /// Streams text a few words at a time, as the model would.
    fn say(&mut self, text: &str) {
        let words: Vec<&str> = text.split_inclusive(' ').collect();
        for (index, chunk) in words.chunks(3).enumerate() {
            self.event(
                if index == 0 { 300 } else { 45 },
                RunEvent::TextDelta {
                    run: self.run.clone(),
                    parent: None,
                    delta: chunk.concat(),
                },
            );
        }
    }

    fn think(&mut self, text: &str) {
        self.event(
            400,
            RunEvent::ThinkingDelta {
                run: self.run.clone(),
                delta: text.into(),
            },
        );
    }

    fn start_tool(&mut self, id: &str, tool: &str, args: Value) {
        self.event(
            350,
            RunEvent::ToolStart {
                run: self.run.clone(),
                call_id: id.into(),
                tool: Arc::from(tool),
                args,
            },
        );
    }

    fn end_tool(&mut self, millis: u64, id: &str, output: ToolOutput) {
        self.event(
            millis,
            RunEvent::ToolEnd {
                run: self.run.clone(),
                call_id: id.into(),
                output: Arc::new(output),
                is_error: false,
            },
        );
    }

    fn tool(
        &mut self,
        millis: u64,
        id: &str,
        tool: &str,
        args: Value,
        output: ToolOutput,
    ) {
        self.start_tool(id, tool, args);
        self.end_tool(millis, id, output);
    }

    fn note(&mut self, millis: u64, note: PluginNote) {
        self.at(millis, RunUpdate::Note(note));
    }
}

fn lines(count: usize) -> ToolOutput {
    ToolOutput::text(
        (1..=count)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn edit(path: &str, new_text: &str, diff: &str) -> (Value, ToolOutput) {
    (
        json!({ "path": path, "edits": [{ "oldText": "", "newText": new_text }] }),
        ToolOutput {
            details: Some(json!({ "diff": diff, "firstChangedLine": 131 })),
            ..ToolOutput::text(format!(
                "Successfully replaced 1 block(s) in {path}."
            ))
        },
    )
}

/// The whole session, from the user's message to the notes memory
/// suggests keeping.
pub fn script() -> Vec<Step> {
    let mut s = Script::new();
    let run = s.run.clone();

    s.at(200, RunUpdate::User(
        "The retry loop ignores `retry-after` on 429s. Honor it, cap it at \
         the policy's max delay, and add tests."
            .into(),
    ));
    s.event(
        300,
        RunEvent::RunStart {
            run: run.clone(),
            parent: None,
            agent: Arc::from("coder"),
        },
    );

    // tau-reasoning and tau-memory run in `start`, before the session.
    s.note(
        500,
        PluginNote {
            plugin: "tau-reasoning".into(),
            text: "picked high reasoning for this run".into(),
            detail: Some("Jev · 180 ms · $0.00002".into()),
            tone: Tone::Info,
            body: NoteBody::Distribution {
                levels: [
                    ("minimal", 0.01),
                    ("low", 0.02),
                    ("medium", 0.09),
                    ("high", 0.84),
                    ("xhigh", 0.04),
                ]
                .into_iter()
                .map(|(name, p)| (name.to_owned(), p))
                .collect(),
                chosen: 3,
                note:
                    "Confidence 0.84 is above 0.70, so the run uses high. It \
                   stays fixed for the whole run."
                        .into(),
            },
        },
    );
    s.note(
        400,
        PluginNote {
            plugin: "tau-memory".into(),
            text: "added 3 notes to the context".into(),
            detail: Some("hybrid search · 21 ms".into()),
            tone: Tone::Info,
            body: NoteBody::Chips(vec![
                "Retry policy honors server hints".into(),
                "429 vs 503 in the Responses API".into(),
                "Tests use the fake OpenAI server".into(),
            ]),
        },
    );
    s.at(
        0,
        RunUpdate::Plan(vec![
            PlanField {
                name: "reasoning".into(),
                value: "high".into(),
                set_by: Some("tau-reasoning".into()),
            },
            PlanField {
                name: "context".into(),
                value: "3 notes".into(),
                set_by: Some("tau-memory".into()),
            },
            PlanField {
                name: "instructions".into(),
                value: "agent default".into(),
                set_by: None,
            },
            PlanField {
                name: "model".into(),
                value: "gpt-5.5".into(),
                set_by: None,
            },
        ]),
    );
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 notes",
            "watching edits",
            "idle · 2% of window",
        ])),
    );

    s.turn();
    s.think("The user wants retry-after honored. Memory has a note on this.");
    s.start_tool("c1", "memory_read", json!({ "id": "n-0388" }));
    s.at(
        0,
        RunUpdate::ToolPlugin {
            call_id: "c1".into(),
            plugin: "tau-memory".into(),
        },
    );
    s.end_tool(
        300,
        "c1",
        ToolOutput::text("Retry policy honors server hints\n\n412 words"),
    );
    s.say(
        "The note says we already decided `retry-after` should win over \
         our own backoff, capped at `max_delay`, and that the header can be \
         seconds or an HTTP date. I'll find where the loop reads it.",
    );
    s.tool(
        250,
        "c2",
        "grep",
        json!({ "pattern": "retry_after|RetryPolicy", "path": "crates/tau-ai/src" }),
        lines(5),
    );
    s.end_turn(9_800, 420, 0.012);

    s.turn();
    s.tool(
        400,
        "c3",
        "read",
        json!({ "path": "crates/tau-ai/src/retry.rs" }),
        lines(188),
    );
    s.say(
        "`RetryPolicy::delay` only knows the attempt number. I'll pass the \
         parsed header in and take the smaller of it and `max_delay`.",
    );
    // tau-constitution blocks the first edit in `before_tool`.
    s.start_tool(
        "c4",
        "edit",
        json!({
            "path": "crates/tau-ai/src/retry.rs",
            "edits": [{
                "oldText": "",
                "newText": "let secs: u64 = header.to_str().unwrap().parse().unwrap();"
            }]
        }),
    );
    let reason = "R2: \"Library code returns errors. No unwrap or expect \
                  outside tests.\"";
    s.end_tool(500, "c4", ToolOutput::text(reason));
    s.at(
        0,
        RunUpdate::Tool {
            call_id: "c4".into(),
            state: ToolState::Blocked {
                plugin: "tau-constitution".into(),
                rule: "R2".into(),
                reason: reason.into(),
                score: "p 0.95 ≥ 0.80".into(),
            },
        },
    );
    s.at(0, RunUpdate::PluginCost(0.00007));
    s.end_turn(21_400, 610, 0.027);

    s.turn();
    s.say(
        "Fair. A malformed header from a proxy shouldn't panic the client. \
         If it doesn't parse, I'll fall back to the policy's own backoff.",
    );
    let (args, output) = edit(
        "crates/tau-ai/src/retry.rs",
        "fn server_hint(...)",
        " fn server_hint(headers: &HeaderMap) -> Option<Duration> {\n\
         +    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;\n\
         +    value.parse::<u64>().ok().map(Duration::from_secs)\n\
         +        .or_else(|| parse_http_date(value))\n\
         -    todo!()\n\
         \x20}\n",
    );
    s.tool(450, "c5", "edit", args, output);
    s.at(
        0,
        RunUpdate::Checks {
            call_id: "c5".into(),
            checks: vec!["R2 0.02".into(), "R4 0.06".into()],
        },
    );
    s.tool(
        600,
        "c6",
        "bash",
        json!({ "command": "rm -rf target/debug/incremental" }),
        ToolOutput::text(""),
    );
    s.at(
        0,
        RunUpdate::Tool {
            call_id: "c6".into(),
            state: ToolState::Flagged {
                plugin: "tau-constitution".into(),
                rule: "R1".into(),
                score: "0.41".into(),
            },
        },
    );
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 notes · 1 read",
            "1 blocked · 1 flagged",
            "idle · 38% of window",
        ])),
    );
    s.end_turn(58_000, 1_900, 0.071);

    // A reviewer sub-agent looks at the parser while the run goes on.
    let reviewer = RunId(Arc::from("retry-after/reviewer"));
    s.event(
        300,
        RunEvent::RunStart {
            run: reviewer.clone(),
            parent: Some(run.clone()),
            agent: Arc::from("reviewer"),
        },
    );

    s.turn();
    s.tool(
        800,
        "c7",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai" }),
        lines(40),
    );
    s.tool(
        300,
        "c8",
        "read",
        json!({ "path": "crates/tau-ai/src/client.rs" }),
        lines(610),
    );
    s.tool(
        300,
        "c9",
        "read",
        json!({ "path": "crates/tau-testing/src/fake_openai.rs" }),
        lines(402),
    );
    s.event(
        400,
        RunEvent::RunEnd {
            run: reviewer,
            parent: Some(run.clone()),
            stop: StopReason::Stop,
            cost: 0.019,
        },
    );
    s.end_turn(96_000, 2_400, 0.063);

    // tau-fast-compaction prunes at the turn boundary.
    s.event(
        700,
        RunEvent::ContextRewritten {
            run: run.clone(),
            plugin: Arc::from("tau-fast-compaction"),
            tokens_before: 172_000,
            tokens_after: 81_000,
        },
    );
    s.at(
        0,
        RunUpdate::RewriteDetail("38 calls judged · $0.0011".into()),
    );
    for (id, pruned) in [
        ("c2", Pruned::Kept),
        ("c3", Pruned::ResultDropped),
        ("c7", Pruned::CallDropped),
        ("c8", Pruned::ResultDropped),
        ("c9", Pruned::ResultDropped),
    ] {
        s.at(
            0,
            RunUpdate::Pruned {
                call_id: id.into(),
                pruned,
            },
        );
    }
    s.at(0, RunUpdate::PluginCost(0.0011));
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 notes · 1 read",
            "1 blocked · 1 flagged",
            "1 rewrite · −91k",
        ])),
    );

    s.turn();
    s.say(
        "Both parsers are in. Now the loop itself: when `server_hint` \
         returns a value, it replaces the backoff for that attempt, capped \
         at `max_delay`.",
    );
    let (args, output) = edit(
        "crates/tau-ai/src/retry.rs",
        "let wait = ...",
        "-let wait = self.policy.delay(attempt);\n\
         +let wait = server_hint(&headers)\n\
         +    .map(|hint| hint.min(self.policy.max_delay))\n\
         +    .unwrap_or_else(|| self.policy.delay(attempt));\n",
    );
    s.tool(450, "c10", "edit", args, output);
    s.start_tool(
        "c11",
        "bash",
        json!({ "command": "cargo nextest run -p tau-ai retry::" }),
    );
    let mut log = String::new();
    for test in [
        "retry::tests::honors_seconds",
        "retry::tests::honors_http_date",
        "retry::tests::caps_at_max_delay",
        "retry::tests::ignores_malformed_header",
    ] {
        log.push_str(&format!("    PASS [0.01s] tau-ai {test}\n"));
        s.event(
            500,
            RunEvent::ToolUpdate {
                run: run.clone(),
                call_id: "c11".into(),
                partial: Arc::new(ToolOutput::text(log.clone())),
            },
        );
    }
    log.push_str("     Summary [3.1s] 14 tests run: 14 passed, 0 skipped");
    s.end_tool(600, "c11", ToolOutput::text(log));
    s.end_turn(81_000, 1_300, 0.052);

    s.turn();
    s.say(
        "Done. `retry-after` now overrides the backoff on 429 and 503, in \
         seconds or as an HTTP date, and is capped at `max_delay`. A header \
         that doesn't parse falls back to the normal backoff. I added four \
         tests against the fake server.",
    );
    s.end_turn(82_000, 700, 0.024);
    // tau-constitution holds the stop in `before_stop`.
    s.event(
        500,
        RunEvent::Continued {
            run: run.clone(),
            plugin: Arc::from("tau-constitution"),
            message: "R6 wants the final answer to name the tests that ran."
                .into(),
        },
    );

    s.turn();
    s.say(
        "Tests: `cargo nextest run -p tau-ai retry::`, 14 passed, 4 of them \
         new: `honors_seconds`, `honors_http_date`, `caps_at_max_delay`, \
         `ignores_malformed_header`.",
    );
    s.end_turn(82_600, 300, 0.012);
    s.at(0, RunUpdate::PluginCost(0.012));
    s.event(
        400,
        RunEvent::RunEnd {
            run: run.clone(),
            parent: None,
            stop: StopReason::Stop,
            cost: 0.274,
        },
    );

    // tau-memory distills in `finish`, after the run is stored.
    s.note(
        900,
        PluginNote {
            plugin: "tau-memory".into(),
            text: "suggests 2 notes from this run".into(),
            detail: Some("finish · distilled with gpt-5.5 · $0.012".into()),
            tone: Tone::Info,
            body: NoteBody::Proposals(vec![
                Proposal {
                    title: "retry-after can be an HTTP date".into(),
                    detail: "Links to Retry policy honors server hints. From \
                         turn 7."
                        .into(),
                },
                Proposal {
                    title: "Proxies send malformed retry-after".into(),
                    detail: "Links to 429 vs 503 in the Responses API. From \
                         turn 5."
                        .into(),
                },
            ]),
        },
    );
    s.at(
        0,
        RunUpdate::Plugins(plugins([
            "chose high",
            "3 in · 2 proposed",
            "1 block · 1 flag · 1 hold",
            "1 rewrite · −91k",
        ])),
    );
    s.steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{Item, ToolState};

    #[test]
    fn the_script_plays_to_a_finished_run() {
        let mut view = retry_after();
        for (_, update) in script() {
            view.update(update);
        }
        assert_eq!(view.status, RunStatus::Finished(StopReason::Stop));
        assert!(matches!(
            view.tool("c4").map(|card| &card.state),
            Some(ToolState::Blocked { .. })
        ));
        assert_eq!(view.children.len(), 1);
        assert!(
            view.items
                .iter()
                .any(|item| matches!(item, Item::Rewrite { .. }))
        );
        assert!(matches!(view.items.last(), Some(Item::Plugin(_))));
    }
}
