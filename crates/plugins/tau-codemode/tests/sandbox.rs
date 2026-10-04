//! The sandbox, as examples: pi's `sandbox.test.ts` list, for Luau.

mod common;

use std::{
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use common::{FakeHost, error, script, script_with, texts};
use serde_json::json;
use tau_codemode::{
    CallStatus,
    CancellationToken,
    Failure,
    Item,
    store::Snapshot,
};

fn host() -> Arc<FakeHost> {
    Arc::new(FakeHost::default())
}

#[tokio::test]
async fn return_values_are_appended_as_json() {
    let outcome = script(
        &host(),
        "return { b = { 1, 2.5 }, a = 'x', empty = array({}), none = json.null }",
    )
    .await;
    assert_eq!(outcome.failure, None);
    assert_eq!(
        texts(&outcome),
        [r#"{"a":"x","b":[1,2.5],"empty":[],"none":null}"#]
    );
}

#[tokio::test]
async fn output_keeps_its_order() {
    let outcome = script(
        &host(),
        "text('a')\nprint('b', 1, nil, true)\nreturn 'c', 2",
    )
    .await;
    assert_eq!(texts(&outcome), ["a", "b\t1\tnil\ttrue", "c", "2"]);
}

#[tokio::test]
async fn errors_carry_their_line() {
    let host = host();
    let outcome = script(&host, "local x = 1\n\nerror('boom')").await;
    assert_eq!(error(&outcome), "codemode:3: boom");

    let outcome = script(&host, "local t = nil\nreturn t.field").await;
    assert_eq!(
        error(&outcome),
        "codemode:2: attempt to index nil with 'field'"
    );

    let outcome = script(&host, "local x =\n\nreturn 1 +").await;
    assert!(error(&outcome).starts_with("codemode:3: "), "{outcome:?}");

    let outcome = script(&host, "\ntools.fail({})").await;
    assert_eq!(error(&outcome), "codemode:2: tool broke");
}

#[tokio::test]
async fn options_line_keeps_line_numbers() {
    let outcome = script(
        &host(),
        "-- @options: {\"max_output_tokens\": 5}\nerror('x')",
    )
    .await;
    assert_eq!(error(&outcome), "codemode:2: x");
}

#[tokio::test]
async fn a_failing_tool_can_be_caught() {
    let outcome = script(
        &host(),
        "local ok, e = pcall(tools.fail, {})\n\
         local ok2, e2 = pcall(function() return tools.fail({}) end)\n\
         return { ok = ok, e = e, ok2 = ok2, e2 = e2 }",
    )
    .await;
    assert_eq!(
        texts(&outcome),
        [r#"{"e":"tool broke","e2":"tool broke","ok":false,"ok2":false}"#]
    );
    let statuses: Vec<_> = outcome.calls.iter().map(|c| c.status).collect();
    assert_eq!(statuses, [CallStatus::Error, CallStatus::Error]);
}

#[tokio::test]
async fn tools_get_their_arguments_and_return_values() {
    let host = host();
    let outcome = script(
        &host,
        "local r = tools.echo({ path = 'a', n = 2, list = array({}) })\n\
         return r.path .. r.n, tools.echo()",
    )
    .await;
    assert_eq!(texts(&outcome), ["a2", "{}"]);
    let calls = host.calls.lock().unwrap().clone();
    assert_eq!(calls[0].args, json!({ "path": "a", "n": 2, "list": [] }));
    assert_eq!(calls[0].id, "call_1/1");
    assert_eq!(calls[1].id, "call_1/2");
}

#[tokio::test]
async fn an_mcp_error_result_is_returned_not_raised() {
    let outcome = script(
        &host(),
        "local r = tools.mcp__linear__list_issues({ team = 'PI' })\n\
         return r.isError, r.structuredContent.count",
    )
    .await;
    assert_eq!(texts(&outcome), ["true", "0"]);
    assert!(!outcome.is_error(), "a readable tool error does not raise");
    assert_eq!(outcome.calls[0].status, CallStatus::Error);
    assert_eq!(outcome.calls[0].error.as_deref(), Some("no access"));
}

#[tokio::test(flavor = "multi_thread")]
async fn parallel_calls_overlap() {
    let host = host();
    let started = Instant::now();
    let outcome = script(
        &host,
        "local a, b, c = parallel(\n\
             function() return tools.sleep({ ms = 300 }) end,\n\
             function() return tools.sleep({ ms = 300 }) end,\n\
             function() return 'plain' end\n\
         )\n\
         return { a, b, c }",
    )
    .await;
    assert_eq!(texts(&outcome), ["[300,300,\"plain\"]"]);
    assert!(started.elapsed() < Duration::from_millis(550));
    assert_eq!(host.peak.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn parallel_raises_the_first_error_after_every_function() {
    let outcome = script(
        &host(),
        "parallel(\n\
             function() tools.fail({}) end,\n\
             function() tools.sleep({ ms = 50 }) text('second done') end\n\
         )",
    )
    .await;
    assert_eq!(error(&outcome), "codemode:2: tool broke");
    assert_eq!(texts(&outcome)[0], "second done");
}

#[tokio::test]
async fn parallel_settled_never_raises() {
    let outcome = script(
        &host(),
        "return parallel_settled(\n\
             function() return tools.fail({}) end,\n\
             function() return 7 end,\n\
             function() error('own') end\n\
         )",
    )
    .await;
    assert_eq!(
        texts(&outcome),
        [
            r#"[{"error":"codemode:2: tool broke","ok":false},{"ok":true,"value":7},{"error":"codemode:4: own","ok":false}]"#
        ]
    );
}

#[tokio::test]
async fn parallel_refuses_what_is_not_a_function() {
    let outcome = script(&host(), "parallel(function() end, 3)").await;
    assert_eq!(
        error(&outcome),
        "codemode:1: parallel: argument 2 is a number, not a function"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn sequential_tools_run_one_at_a_time() {
    let host = host();
    let started = Instant::now();
    let outcome = script(
        &host,
        "parallel(\n\
             function() return tools.slow_one({ ms = 150 }) end,\n\
             function() return tools.slow_one({ ms = 150 }) end\n\
         )",
    )
    .await;
    assert_eq!(outcome.failure, None);
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(host.peak.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_tight_loop_times_out() {
    let started = Instant::now();
    let outcome = script(
        &host(),
        "-- @options: {\"timeout_ms\": 200}\nwhile true do end",
    )
    .await;
    assert_eq!(outcome.failure, Some(Failure::TimedOut { timeout_ms: 200 }));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(error(&outcome).starts_with("Script timed out: "));
}

#[tokio::test]
async fn pcall_cannot_swallow_the_timeout() {
    let outcome = script(
        &host(),
        "-- @options: {\"timeout_ms\": 100}\n\
         while true do pcall(function() while true do end end) end",
    )
    .await;
    assert_eq!(outcome.failure, Some(Failure::TimedOut { timeout_ms: 100 }));
}

/// One worker thread: the cancel task runs only if the loop yields.
#[tokio::test(flavor = "current_thread")]
async fn cancel_stops_a_tight_loop_without_holding_the_worker() {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let ticker = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let outcome = script_with(
        &host(),
        "local n = 0\nwhile true do n = n + 1 end",
        Snapshot::new(),
        cancel,
    )
    .await;
    ticker.await.unwrap();
    assert_eq!(outcome.failure, Some(Failure::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn cancel_stops_a_waiting_call() {
    let host = host();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(50)).await;
        trigger.cancel();
    });
    let outcome = script_with(
        &host,
        "tools.sleep({ ms = 10000 })",
        Snapshot::new(),
        cancel,
    )
    .await;
    assert_eq!(outcome.failure, Some(Failure::Cancelled));
    assert_eq!(outcome.calls[0].status, CallStatus::Cancelled);
    assert_eq!(host.dropped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn the_memory_limit_fails_the_script() {
    let outcome = script(
        &host(),
        "local t = {}\nfor i = 1, 1e9 do t[i] = string.rep('x', 4096) .. i end",
    )
    .await;
    assert!(error(&outcome).contains("not enough memory"), "{outcome:?}");
}

#[tokio::test]
async fn the_sandbox_has_no_way_out() {
    let outcome = script(
        &host(),
        "return {\n\
             io = io == nil,\n\
             execute = os.execute == nil,\n\
             exit = os.exit == nil,\n\
             require = type(require) == 'function',\n\
             package = package == nil,\n\
             dofile = dofile == nil,\n\
             loadfile = loadfile == nil,\n\
             dump = string.dump == nil,\n\
         }",
    )
    .await;
    assert_eq!(
        texts(&outcome),
        [
            r#"{"dofile":true,"dump":true,"execute":true,"exit":true,"io":true,"loadfile":true,"package":true,"require":true}"#
        ]
    );
}

#[tokio::test]
async fn globals_are_read_only() {
    let host = host();
    for code in [
        "tools.echo = nil",
        "tools.extra = function() end",
        "string.upper = nil",
        "json.null = 1",
        "ALL_TOOLS[1].name = 'x'",
    ] {
        let outcome = script(&host, code).await;
        assert!(
            error(&outcome).contains("attempt to modify a readonly table"),
            "{code}: {outcome:?}"
        );
    }
    // A script's own globals shadow; they never reach the host.
    let outcome =
        script(&host, "text = nil\nreturn tools.echo({ a = 1 }).a").await;
    assert_eq!(texts(&outcome), ["1"]);
}

#[tokio::test]
async fn calls_still_running_at_the_end_are_cancelled() {
    let host = host();
    let outcome = script(
        &host,
        "parallel(\n\
             function() tools.sleep({ ms = 10000 }) end,\n\
             function() tools.sleep({ ms = 10 }) exit() end\n\
         )",
    )
    .await;
    assert_eq!(outcome.failure, None);
    let statuses: Vec<_> = outcome.calls.iter().map(|c| c.status).collect();
    assert_eq!(statuses, [CallStatus::Cancelled, CallStatus::Ok]);
    assert_eq!(host.dropped.load(Ordering::SeqCst), 1);

    let outcome = script(
        &host,
        "-- @options: {\"timeout_ms\": 100}\ntools.sleep({ ms = 10000 })",
    )
    .await;
    assert_eq!(outcome.failure, Some(Failure::TimedOut { timeout_ms: 100 }));
    assert_eq!(outcome.calls[0].status, CallStatus::Cancelled);
    assert_eq!(host.dropped.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn exit_keeps_output_and_writes() {
    let outcome =
        script(&host(), "text('a')\nstore('k', 1)\npcall(exit)\ntext('b')")
            .await;
    assert_eq!(outcome.failure, None);
    assert_eq!(texts(&outcome), ["a"]);
    assert_eq!(outcome.store.unwrap().set["k"], json!(1));
}

#[tokio::test]
async fn a_failed_script_keeps_its_output_and_writes_nothing() {
    let outcome = script(
        &host(),
        "text('partial')\nstore('k', 1)\ntools.echo({})\ntools.fail({})",
    )
    .await;
    assert_eq!(outcome.store, None);
    assert_eq!(texts(&outcome), ["partial"]);
    assert_eq!(
        outcome.failure_text().unwrap(),
        "Script error:\ncodemode:4: tool broke\n\nTool calls made before the \
         failure (they are not undone): echo (ok), fail (error)"
    );
    let outcome = script(&host(), "error('x')").await;
    assert!(
        outcome
            .failure_text()
            .unwrap()
            .ends_with("No tool calls were made.")
    );
}

#[tokio::test]
async fn a_return_value_json_cannot_hold_fails_the_script() {
    let outcome = script(&host(), "return function() end").await;
    assert_eq!(
        error(&outcome),
        "The script's return value: a function cannot be encoded as JSON"
    );
}

#[tokio::test]
async fn the_store_loads_copies_and_checks_sizes() {
    let snapshot: Snapshot = [("k".to_owned(), json!({ "a": 1 }))].into();
    let outcome = script_with(
        &host(),
        "local v = load('k')\nv.a = 2\n\
         store('n', { list = array({}) })\nstore('k', nil)\n\
         return load('k') == nil, load('n'), load('missing') == nil",
        snapshot,
        CancellationToken::new(),
    )
    .await;
    assert_eq!(texts(&outcome), ["true", r#"{"list":[]}"#, "true"]);
    let writes = outcome.store.unwrap();
    assert_eq!(writes.set["n"], json!({ "list": [] }));
    assert_eq!(writes.delete, ["k"]);

    let outcome =
        script(&host(), "store('big', string.rep('x', 300 * 1024))").await;
    assert!(
        error(&outcome).starts_with("codemode:1: store: the value for `big`")
    );
    let outcome = script(&host(), "store(1, 2)").await;
    assert_eq!(
        error(&outcome),
        "codemode:1: store(): the key must be a string"
    );
}

#[tokio::test]
async fn images_are_checked_and_typed_by_their_bytes() {
    // A PNG signature, declared as JPEG, wrapped over two lines.
    let png = "iVBORw0K\nGgo=";
    let outcome = script(
        &host(),
        &format!(
            "image('data:image/jpeg;base64,{}')\n\
             image({{ type = 'image', data = 'R0lGODlh', mimeType = 'image/png' }})",
            png.replace('\n', "\\n")
        ),
    )
    .await;
    assert_eq!(outcome.failure, None, "{outcome:?}");
    let types: Vec<_> = outcome
        .items
        .iter()
        .map(|item| match item {
            Item::Image(image) => image.mime_type,
            Item::Text(_) | Item::Json(_) => "text",
        })
        .collect();
    assert_eq!(types, ["image/png", "image/gif"]);

    let outcome = script(&host(), "image('https://example.com/a.png')").await;
    assert!(error(&outcome).contains("cannot fetch URLs"));
    let outcome =
        script(&host(), "image('data:image/png;base64,aGVsbG8=')").await;
    assert!(error(&outcome).contains("not a PNG, JPEG, GIF or WebP"));
}

#[tokio::test]
async fn coroutines_of_the_script_are_not_disturbed() {
    let outcome = script(
        &host(),
        "local co = coroutine.wrap(function()\n\
             local s = 0\n\
             for i = 1, 200000 do s = s + i end\n\
             coroutine.yield(s)\n\
             return 'second'\n\
         end)\n\
         return co(), co()",
    )
    .await;
    assert_eq!(texts(&outcome), ["20000100000", "second"]);
}

#[tokio::test]
async fn discovery_finds_tools_and_namespaces() {
    let outcome = script(
        &host(),
        "local found = search_tools('linear issues')\n\
         local ns = describe_namespace('linear')\n\
         return #ALL_TOOLS, found[1].name, ns.instructions, ns.tools,\n\
             describe_tool('nope') == nil,\n\
             #search_tools('wait', { limit = 1 }),\n\
             #search_tools('issues', { namespace = 'other' })",
    )
    .await;
    assert_eq!(
        texts(&outcome),
        [
            "5",
            "mcp__linear__list_issues",
            "Use team keys such as PI.",
            r#"["mcp__linear__list_issues"]"#,
            "true",
            "1",
            "0"
        ]
    );
    let outcome =
        script(&host(), "return describe_tool('mcp__linear__list_issues')")
            .await;
    let text = &texts(&outcome)[0];
    assert!(text.starts_with(
        "-- List Linear issues.\nfunction tools.mcp__linear__list_issues(args: { team: string? }): CallToolResult<{ count: number }>"
    ), "{text}");
    assert!(text.contains("type CallToolResult<T> ="));
}

#[tokio::test]
async fn jev_is_nil_without_one() {
    let outcome = script(&host(), "return jev == nil").await;
    assert_eq!(texts(&outcome), ["true"]);
}

#[tokio::test]
async fn render_has_the_header_and_cuts_long_output() {
    let outcome = script(
        &host(),
        "-- @options: {\"max_output_tokens\": 10}\n\
         for i = 1, 100 do text('line ' .. i) end",
    )
    .await;
    let rendered = outcome.render(10);
    assert!(!rendered.is_error);
    let Item::Text(header) = &rendered.content[0] else {
        panic!("a text header");
    };
    assert!(header.starts_with("Script completed\nWall time "));
    assert!(header.ends_with(" seconds\nOutput:\n"));
    let Item::Text(body) = &rendered.content[1] else {
        panic!("a text body");
    };
    assert!(
        body.starts_with("Warning: truncated output (original token count: ")
    );
    assert!(body.contains("Total output lines: 100\n\nline 1\nline"));
    let path = body
        .rsplit("[Full output: ")
        .next()
        .unwrap()
        .split(" (read it")
        .next()
        .unwrap();
    assert!(path.contains("tau-codemode-"));
    let full = std::fs::read_to_string(path).unwrap();
    assert!(full.ends_with("line 100"));
    std::fs::remove_file(path).unwrap();
    assert_eq!(rendered.details["complete"], json!(true));
    assert_eq!(rendered.content.len(), 2);
}

#[tokio::test]
async fn a_failed_render_is_an_error() {
    let rendered = script(&host(), "tools.echo({ a = 1 })\nerror('x')")
        .await
        .render(10_000);
    assert!(rendered.is_error);
    let Item::Text(header) = &rendered.content[0] else {
        panic!("a text header");
    };
    assert!(header.starts_with("Script failed\n"));
    assert_eq!(rendered.details["calls"][0]["name"], json!("echo"));
    assert_eq!(rendered.details["calls"][0]["args"], json!(r#"{"a":1}"#));
    assert_eq!(rendered.details["store"], json!(null));
}

#[tokio::test]
async fn rows_stop_at_the_cap() {
    let outcome =
        script(&host(), "for i = 1, 300 do tools.echo({}) end\nerror('x')")
            .await;
    assert_eq!(outcome.calls.len(), 256);
    assert_eq!(outcome.calls_total, 300);
    assert_eq!(outcome.details()["complete"], json!(false));
    assert!(
        outcome
            .failure_text()
            .unwrap()
            .contains("echo (ok), and 44 more")
    );
}
