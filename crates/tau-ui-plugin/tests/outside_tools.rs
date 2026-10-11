//! The tools a host half calls outside any run (ADR 0037): each name
//! goes to the first source that has it.

#![allow(
    clippy::disallowed_methods,
    reason = "a test is a synchronous entry point (ADR 0028)"
)]

use std::{path::Path, sync::Arc};

use serde_json::{Value, json};
use tau_ui_plugin::{OutsideTools, ToolSource, registry::HostFuture};

/// Has the tools whose names start with `prefix`, and answers with
/// where it was asked and what.
struct Prefixed(&'static str);

impl ToolSource for Prefixed {
    fn call<'a>(
        &'a self,
        checkout: &'a Path,
        name: &'a str,
        args: Value,
    ) -> Option<HostFuture<'a, Result<Value, String>>> {
        name.starts_with(self.0).then(|| {
            let answer = json!({
                "source": self.0,
                "checkout": checkout.display().to_string(),
                "name": name,
                "args": args,
            });
            Box::pin(async move { Ok(answer) }) as HostFuture<'a, _>
        })
    }
}

fn block_on<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn a_tool_goes_to_the_source_that_has_it() {
    let tools = OutsideTools::default();
    tools.offer(Arc::new(Prefixed("mcp__")));
    // A clone is the same set of sources.
    tools.clone().offer(Arc::new(Prefixed("")));
    let answer = block_on(tools.call(
        Path::new("/agents/a"),
        "mcp__github__list",
        json!({ "state": "open" }),
    ))
    .unwrap();
    assert_eq!(answer["source"], "mcp__");
    assert_eq!(answer["checkout"], "/agents/a");
    assert_eq!(answer["args"], json!({ "state": "open" }));
    let other =
        block_on(tools.call(Path::new("/a"), "read", json!({}))).unwrap();
    assert_eq!(other["source"], "");
}

#[test]
fn a_tool_no_source_has_says_so() {
    let tools = OutsideTools::default();
    tools.offer(Arc::new(Prefixed("mcp__")));
    let error =
        block_on(tools.call(Path::new("/a"), "bash", json!({}))).unwrap_err();
    assert!(error.contains("`bash`"), "{error}");
}
