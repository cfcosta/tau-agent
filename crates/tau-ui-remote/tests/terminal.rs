//! `bash` cards under a terminal: tau-tools builds a card's terminal
//! from the `term` chunks and the result's details the view keeps, live
//! and from history; the workspace draws it and copies it.

use std::sync::Arc;

use base64::Engine as _;
use gpui::{TestAppContext, VisualTestContext};
use hegel::{TestCase, generators as gs};
use serde_json::{Value, json};
use tau_agent::{
    event::RunEvent,
    tool::{RunId, ToolOutput},
};
use tau_ai::message::Message;
use tau_tools::ui::{
    term::{SeenLine, TermOutput, TermStatus},
    term_card::TermCards,
};
use tau_ui_remote::{
    Workspace,
    catalog::Catalog,
    route::Route,
    view::{RunView, Stored, ToolCard, ToolState},
};

fn run() -> RunId {
    RunId(Arc::from("r"))
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn chunk(seq: u64, bytes: &[u8]) -> RunEvent {
    RunEvent::ToolUpdate {
        run: run(),
        call_id: "c1".into(),
        partial: Arc::new(ToolOutput {
            details: Some(json!({"term": {"seq": seq, "bytes": b64(bytes)}})),
            ..ToolOutput::text("so far")
        }),
        parent: None,
    }
}

fn result_details(bytes: &[u8], code: Option<i32>, status: &str) -> Value {
    json!({"term": {
        "cols": 120, "rows": 40, "status": status, "exitCode": code,
        "chunks": 2, "outputBytes": bytes.len(), "replay": "stream",
        "bytes": b64(bytes),
    }})
}

fn started() -> RunView {
    let mut view = RunView::new(run(), "t", "coder", "m");
    view.apply(&RunEvent::ToolStart {
        run: run(),
        call_id: "c1".into(),
        tool: "bash".into(),
        args: json!({"command": "cargo test"}),
        parent: None,
    });
    view
}

fn card(view: &RunView) -> &ToolCard {
    view.tool("c1").expect("the card")
}

/// The terminal tau-tools draws for the card, folded from scratch.
fn term(view: &RunView) -> Option<TermOutput> {
    let card = card(view);
    TermCards::default().output(
        &(run(), card.call_id.clone()),
        &card.data,
        card.cut.is_some(),
    )
}

/// Chunks join in `seq` order, whatever else arrives: a repeated or
/// early chunk is dropped, and an update with text only changes
/// nothing.
#[hegel::test(test_cases = 100)]
fn chunks_join_in_order(tc: TestCase) {
    let pieces: Vec<Vec<u8>> = tc.draw(
        gs::vecs(gs::vecs(gs::integers::<u8>()).min_size(1).max_size(8))
            .min_size(1)
            .max_size(8),
    );
    // Each chunk is sent, then some earlier or later one again.
    let mut view = started();
    for (seq, piece) in pieces.iter().enumerate() {
        view.apply(&chunk(seq as u64, piece));
        let other: usize =
            tc.draw(gs::integers::<usize>().max_value(pieces.len() - 1));
        view.apply(&chunk(other as u64, &pieces[other]));
        view.apply(&RunEvent::ToolUpdate {
            run: run(),
            call_id: "c1".into(),
            partial: Arc::new(ToolOutput::text("text only")),
            parent: None,
        });
    }
    let term = term(&view).expect("a terminal");
    assert_eq!(*term.bytes, pieces.concat());
    assert_eq!(term.next_seq, pieces.len() as u64);
    assert_eq!(term.end, None);
}

/// The result replaces the chunks with its replay, says how the command
/// ended, and keeps the model's text; a failed exit is the card's
/// failure.
#[test]
fn the_result_ends_the_terminal() {
    let mut view = started();
    view.apply(&chunk(0, b"\x1b[31mFAIL\x1b[0m\r\n"));
    let bytes = b"\x1b[31mFAIL\x1b[0m\r\nerror\r\n";
    view.apply(&RunEvent::ToolEnd {
        run: run(),
        call_id: "c1".into(),
        output: Arc::new(ToolOutput {
            details: Some(result_details(bytes, Some(100), "exited")),
            ..ToolOutput::text("FAIL\nerror\n\nCommand exited with code 100")
        }),
        is_error: true,
        parent: None,
    });
    assert!(matches!(card(&view).state, ToolState::Failed(_)));
    let term = term(&view).expect("a terminal");
    assert_eq!(term.failure().as_deref(), Some("exit 100"));
    assert_eq!(term.bytes.as_slice(), bytes);
    let end = term.end.expect("ended");
    assert_eq!(
        (end.status, end.exit_code, end.snapshot),
        (TermStatus::Exited, Some(100), false)
    );
    assert!(term.seen.starts_with("FAIL\nerror"));
    assert_eq!(term.size_label(), "120×40");
}

/// A timeout says so; a clean exit is done.
#[test]
fn how_a_command_ended_shows_on_its_card() {
    for (code, status, failure) in [
        (None, "timedOut", Some("timed out")),
        (None, "cancelled", Some("cancelled")),
        (Some(0), "exited", None),
    ] {
        let mut view = started();
        view.apply(&RunEvent::ToolEnd {
            run: run(),
            call_id: "c1".into(),
            output: Arc::new(ToolOutput {
                details: Some(result_details(b"ok\r\n", code, status)),
                ..ToolOutput::text("ok")
            }),
            is_error: code != Some(0),
            parent: None,
        });
        assert_eq!(term(&view).unwrap().failure().as_deref(), failure);
        if failure.is_none() {
            assert_eq!(
                card(&view).state,
                ToolState::Done {
                    summary: Some("1 line".into())
                }
            );
        }
    }
}

fn message(value: Value) -> Message {
    serde_json::from_value(value).unwrap()
}

fn history(result: Value) -> RunView {
    let usage = json!({"input": 0, "output": 0, "cacheRead": 0,
        "cacheWrite": 0, "totalTokens": 0,
        "cost": {"input": 0, "output": 0, "cacheRead": 0,
                 "cacheWrite": 0, "total": 0}});
    RunView::from_timeline(
        run(),
        "t",
        "coder",
        "m",
        &[
            Stored::Message(message(json!({
                "role": "user", "content": "test it", "timestamp": 0
            }))),
            Stored::Message(message(json!({
                "role": "assistant", "api": "responses", "provider": "openai",
                "model": "m", "usage": usage, "stopReason": "toolUse",
                "timestamp": 0,
                "content": [{"type": "toolCall", "id": "c1", "name": "bash",
                             "arguments": {"command": "cargo test"}}],
            }))),
            Stored::Message(message(result)),
        ],
    )
}

/// A reopened run rebuilds the terminal from the stored result; a
/// result with no `term` details keeps the plain lines.
#[test]
fn a_stored_result_rebuilds_the_terminal() {
    let bytes = b"\x1b[32mok\x1b[0m\r\n";
    let view = history(json!({
        "role": "toolResult", "toolCallId": "c1", "toolName": "bash",
        "content": [{"type": "text", "text": "ok"}],
        "details": result_details(bytes, Some(0), "exited"),
        "isError": false, "timestamp": 0
    }));
    assert_eq!(term(&view).expect("a terminal").bytes.as_slice(), bytes);

    let old = history(json!({
        "role": "toolResult", "toolCallId": "c1", "toolName": "bash",
        "content": [{"type": "text", "text": "a\nb"}],
        "isError": false, "timestamp": 0
    }));
    assert!(term(&old).is_none());
    assert_eq!(tau_tools::ui::output_lines(&card(&old).data), ["a", "b"]);
}

/// Cut, the model's text reads back the cut's header, omissions and
/// footer; other text is lines.
#[test]
fn the_model_text_marks_what_was_left_out() {
    let mut view = started();
    let seen = format!(
        "{}\nStarting\n[1960 lines omitted]\nSummary\n\n[full output: /a.txt (read or grep it if needed)]",
        tau_fast_compaction::output::HEADER
    );
    view.apply(&RunEvent::ToolEnd {
        run: run(),
        call_id: "c1".into(),
        output: Arc::new(ToolOutput {
            details: Some(result_details(b"x\r\n", Some(0), "exited")),
            ..ToolOutput::text(seen)
        }),
        is_error: false,
        parent: None,
    });
    view.update(tau_ui_remote::view::RunUpdate::Event(
        RunEvent::PluginReport {
            run: run(),
            plugin: tau_fast_compaction::NAME.into(),
            body: json!({
                "kind": "output", "call_id": "c1", "lines": 1964, "chunks": 3,
                "kept": 2, "dropped_lines": 1960, "segments": 1, "requests": 1,
                "tokens_before": 9000, "tokens_after": 40, "pruned": true,
                "archive": "/a.txt",
            }),
        },
    ));
    let lines = term(&view).expect("a terminal").seen_lines();
    assert!(matches!(lines[0], SeenLine::Note(_)));
    assert_eq!(lines[1], SeenLine::Text("Starting".into()));
    assert_eq!(lines[2], SeenLine::Omitted(1960));
    assert!(
        matches!(lines.last(), Some(SeenLine::Note(line)) if line.starts_with("[full output: "))
    );

    // Unpruned text is all lines, even one that looks like a mark.
    let mut plain = started();
    plain.apply(&RunEvent::ToolEnd {
        run: run(),
        call_id: "c1".into(),
        output: Arc::new(ToolOutput {
            details: Some(result_details(b"x\r\n", Some(0), "exited")),
            ..ToolOutput::text("[3 lines omitted]")
        }),
        is_error: false,
        parent: None,
    });
    assert_eq!(
        term(&plain).expect("a terminal").seen_lines(),
        [SeenLine::Text("[3 lines omitted]".into())]
    );
}

/// Drawn in the workspace, a finished card's terminal copies its text,
/// and live chunks reach its screen.
#[gpui::test]
fn the_workspace_draws_and_copies_a_terminal(cx: &mut TestAppContext) {
    cx.update(tau_ui_remote::init);
    let mut view = started();
    view.apply(&chunk(0, b"\x1b[1;32m   Compiling\x1b[0m tau\r\n"));
    let window = cx.add_window(|window, cx| {
        let mut ws =
            Workspace::new("tau", vec![view], Catalog::default(), window, cx);
        ws.navigate(Route::Run(run()), cx);
        ws
    });
    let workspace = window.root(cx).unwrap();
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    cx.run_until_parked();

    workspace.update(&mut cx, |ws, cx| {
        ws.apply_event(&chunk(1, b"    Finished\r\n"), cx);
        let bytes = b"\x1b[1;32m   Compiling\x1b[0m tau\r\n    Finished\r\n";
        ws.apply_event(
            &RunEvent::ToolEnd {
                run: run(),
                call_id: "c1".into(),
                output: Arc::new(ToolOutput {
                    details: Some(result_details(bytes, Some(0), "exited")),
                    ..ToolOutput::text("   Compiling tau\n    Finished\n")
                }),
                is_error: false,
                parent: None,
            },
            cx,
        );
    });
    cx.run_until_parked();
    let cards = workspace.read_with(&cx, |ws, _| {
        ws.plugin_ui::<tau_tools::ui::Ui>(tau_tools::ui::NAME)
            .expect("tau-tools draws its cards")
    });
    let key = (run(), "c1".to_owned());
    cards.update(&mut cx, |cards, cx| cards.terms.copy(&key, cx));
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("   Compiling tau\n    Finished")
    );
    // Expanding and switching tabs are plain toggles.
    cards.update(&mut cx, |cards, cx| {
        cards.terms.toggle_expanded(&key, cx);
        cards.terms.show_model_text(&key, true);
    });
    cx.run_until_parked();
    workspace.read_with(&cx, |ws, _| {
        assert!(term(ws.run(&run()).unwrap()).is_some());
    });
}
