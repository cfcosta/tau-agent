# Libraries for MCP and Codemode

- Status: research, for [0018](../decisions/0018-codemode-and-mcp.md).
- Date: 2026-10-01

## Recommendation

| Need                          | Pick                                            | Version                                                           | License                     |
| ----------------------------- | ----------------------------------------------- | ----------------------------------------------------------------- | --------------------------- |
| MCP client                    | `rmcp` (official modelcontextprotocol/rust-sdk) | 3.5.0 (2026-09-28)                                                | Apache-2.0                  |
| Lua engine                    | `mlua` with `luau`, `async`, `send`, `serde`    | 0.12.1 (2026-08-29), Luau 0.736 (luau0-src 0.22.0+luau740 is out) | MIT (mlua, Luau, luau0-src) |
| JSON Schema -> signature text | Write it by hand (~200 lines)                   | -                                                                 | -                           |

Use Luau, not Lua 5.4:

- `Lua::sandbox(true)` is a real sandbox.
- `set_interrupt` gives a CPU deadline and also a cooperative yield point for tokio.
- Luau's standard library has no `io`, no `os.execute`, no `require` and no `package`.
- Luau parses type annotations. That lets us show tools to the model as typed Luau signatures, and model-written annotations will not cause syntax errors. Lua 5.4 would reject them.
- mlua always builds Luau from the vendored source, so there is no system library or pkg-config step.

## 1. MCP client crates

### rmcp (official)

- crates.io: https://crates.io/crates/rmcp. Versions: 3.5.0 (2026-09-28), 3.4.1, 3.4.0, 3.3.0, 3.2.0 (Aug-Sep 2026). That is a fast release pace, so pin the minor version. About 16M recent downloads.
- Repo: https://github.com/modelcontextprotocol/rust-sdk. Workspace: Apache-2.0, edition 2024, MSRV 1.88.
- Protocol: implements the stable **2026-07-28** spec (stateless HTTP, no `initialize` on new servers, MRTR, list caching, Tasks extension SEP-2663, `subscriptions/listen`). It also falls back to **2025-11-25**. `ProtocolVersion::LATEST = V_2026_07_28`, `LATEST_WITH_INITIALIZE = V_2025_11_25`. `ClientLifecycleMode::Auto { preferred_versions, legacy_version }` negotiates the version.
- Default features are `["base64", "macros", "server"]`. Set **`default-features = false`**.
- Feature flags for a client (from crates/rmcp/Cargo.toml):
  - `client` (adds only tokio-stream)
  - `transport-child-process`: `TokioChildProcess`, built on `process-wrap` 10 and tokio/process. Use `which-command` to resolve the command on PATH.
  - `transport-streamable-http-client-reqwest`: `StreamableHttpClientTransport`. It pulls `sse-stream`, `http`, `bytes`, and reqwest 0.13.2 with `json` and `stream`.
  - `reqwest-tls-no-provider` (reqwest `rustls-no-provider`). This **matches tau's workspace reqwest 0.13.5 with `rustls-no-provider`**, so no second TLS stack is added. Do not use plain `reqwest`: it turns on reqwest `rustls` with its default provider.
  - `auth` (OAuth2 for remote servers; pulls `oauth2` 5 and `url`). Avoid `auth-client-credentials-jwt`: it pulls `jsonwebtoken` with `aws_lc_rs`, which needs a heavier native build.
  - Also available: `elicitation`, `transport-streamable-http-client-unix-socket` (hyper).
- Always-on dependencies are light: serde, serde_json, thiserror, tokio (sync, macros, rt, time), futures, indexmap, tracing, tokio-util, pin-project-lite, chrono (no default features).
- API:
  - `().serve(transport)` or `ClientConfig::new(caps, Implementation::new(..)).serve_with_lifecycle(transport, mode)` returns `RunningService<RoleClient, _>`.
  - `list_tools(Default::default())` returns one page. `list_all_tools()` walks every page.
  - `call_tool(CallToolRequestParams::new("name").with_arguments(map))`.
  - `peer_info()`, and `cancel().await` to shut down. Dropping the service does not stop it at once: a DropGuard cancels it in the background. Call `cancel()` explicitly so the child process exits.
- `Tool` has `name`, `title`, `description`, `input_schema`, `output_schema`, and `annotations`. `CallToolResult` has `content`, `structured_content`, and `is_error`. So structuredContent and outputSchema are supported. `CallToolRequestParams` also carries MRTR fields (`input_responses`, `request_state`).
- Cancellation and timeouts:
  - `peer.send_cancellable_request(req, PeerRequestOptions { timeout, meta, reset_timeout_on_progress, max_total_timeout })` returns a `RequestHandle`.
  - `handle.await_response()` and `handle.cancel(reason)` send `notifications/cancelled`.
  - Plain `call_tool` has no time limit. Use the handle form and connect it to tau's `ToolCtx::cancel` token.
  - Cancellation is best effort: the server can still finish the side effect.
  - There is a known server-side issue (#857), but it does not affect us as a client.
- Possible concern: the API changes often across major versions (several majors in about 18 months). Keep the plugin behind a thin adapter.

Minimal client sketch (rmcp 3.5):

```rust
// Cargo.toml
// rmcp = { version = "3.5", default-features = false, features = [
//   "client", "transport-child-process",
//   "transport-streamable-http-client-reqwest", "reqwest-tls-no-provider",
// ] }
use rmcp::{
    ClientLifecycleMode, ClientServiceExt, ServiceExt,
    model::{CallToolRequestParams, ClientCapabilities, ClientConfig, Implementation, ProtocolVersion},
    service::PeerRequestOptions,
    transport::{ConfigureCommandExt, StreamableHttpClientTransport, TokioChildProcess},
};
use tokio::process::Command;

async fn stdio() -> anyhow::Result<()> {
    let client = ()
        .serve(TokioChildProcess::new(Command::new("uvx").configure(|c| { c.arg("mcp-server-git"); }))?)
        .await?;
    let tools = client.list_all_tools().await?;            // Vec<Tool>: input_schema, output_schema
    let res = client
        .call_tool(CallToolRequestParams::new("git_status")
            .with_arguments(serde_json::json!({"repo_path": "."}).as_object().unwrap().clone()))
        .await?;                                            // res.structured_content / res.content / res.is_error
    client.cancel().await?;                                 // kill the child cleanly
    Ok(())
}

async fn http() -> anyhow::Result<()> {
    let transport = StreamableHttpClientTransport::from_uri("https://example/mcp");
    let client = ClientConfig::new(ClientCapabilities::default(), Implementation::new("tau", "0.1.0"))
        .serve_with_lifecycle(transport, ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        })
        .await?;
    // Cancellable call, connected to tau's ToolCtx::cancel:
    // let h = client.peer().send_cancellable_request(ClientRequest::CallToolRequest(..), PeerRequestOptions { timeout: Some(d), ..Default::default() }).await?;
    // tokio::select! { r = h.await_response() => .., _ = ctx.cancel.cancelled() => h.cancel(None).await? }
    // (Exact request-wrapping: check docs.rs/rmcp/3.5.0 Peer::send_cancellable_request; RequestHandle::cancel consumes the handle,
    //  so in select! keep peer + id or use the timeout option instead.)
    client.cancel().await?;
    Ok(())
}
```

### Alternatives

- `rust-mcp-sdk` 2.0.0 (MIT, 2026-08-27, about 110k recent downloads). Built on `rust-mcp-schema`. Smaller user base, and it is not the reference SDK.
- `tower-mcp` 0.23.2 (MIT OR Apache-2.0, 2026-09-29, about 20k recent downloads). Built on Tower and still pre-1.0.
- Neither is a better fit. rmcp is the official SDK, is the first to support new spec versions (2026-07-28), and its reqwest/rustls feature set matches tau's.

## 2. Lua embedding: mlua 0.12.1

- Repo: https://github.com/mlua-rs/mlua. Docs: https://docs.rs/mlua/0.12.1. MIT. Changelog: https://github.com/mlua-rs/mlua/blob/main/CHANGELOG.md
- 0.12 requires **Rust 2024 edition** (tau is on 2024). The module layout changed: `mlua::{chunk, function, thread, luau, ...}`, and the root re-exports less.
- Backends: `lua55` (5.5.1), `lua54`, `lua53`, `lua52`, `lua51`, `luajit`, `luajit52`, `luau`, `luau-jit` (Luau native codegen), `luau-vector4`.
- Dependencies are small: bstr, either, num-traits, rustc-hash, parking_lot, libc, and futures-util with `async`. Optional: serde, erased-serde, serde-value.

### Backend comparison

|                  | Lua 5.4/5.5                                                                                                                                                            | LuaJIT                                      | Luau                                                                                                                                                                                                                                                         |
| ---------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Sandboxing       | Hand-built: `Lua::new_with(StdLib::TABLE \| STRING \| MATH \| UTF8 \| COROUTINE, LuaOptions::default())`, then strip globals (`load`, `dofile`, `collectgarbage`, ...) | Same, and `ffi` is refused by `new_with`    | `lua.sandbox(true)`: libraries and built-in metatables become read-only, globals go to a per-thread proxy (safeenv), GC allows only `count`. The standard library already lacks io, os.execute, package, require, and string.dump. Load bytecode is checked. |
| CPU limit        | `set_hook(HookTriggers::every_nth_instruction(n), ..)` returns `Err` or `VmState::Yield`                                                                               | hooks are unreliable under JIT              | `set_interrupt(\|lua\| ..)`. It runs at calls and loop back-edges, returns `VmState::Continue`, `VmState::Yield`, or `Err(..)` to abort                                                                                                                      |
| Memory limit     | `set_memory_limit(bytes)` gives `Error::MemoryError`                                                                                                                   | not available (`MemoryControlNotAvailable`) | `set_memory_limit` works                                                                                                                                                                                                                                     |
| Type annotations | syntax error                                                                                                                                                           | syntax error                                | parsed and ignored at run time; mlua does not expose the type checker                                                                                                                                                                                        |
| Build            | `vendored` needs a C compiler (lua-src 551.x)                                                                                                                          | vendored with luajit-src                    | always vendored from `luau0-src`, which needs a **C++** compiler (cc crate)                                                                                                                                                                                  |

`set_memory_limit` and the `sandbox` and interrupt APIs come from the 0.12.1 source of `state.rs`: https://docs.rs/mlua/0.12.1/src/mlua/state.rs.html

### Async with tokio

- `features = ["async"]`:
  - `lua.create_async_function(|lua, args: A| async move { ... Ok(r) })`. The signature is `F: Fn(Lua, A) -> FR`, `FR: Future<Output = Result<R>>`.
  - Inside Lua the call looks synchronous. mlua runs the chunk in a coroutine and yields to the executor while the Rust future is pending.
  - Drive a script with `chunk.eval_async::<T>().await` / `exec_async()`, or `lua.create_thread(f)?.into_async::<R>(args)?`. `AsyncThread` is both a Future and a Stream.
- **`send` is required for tau.** `AgentTool::call` is `#[async_trait]` with Send futures (crates/tau-agent/src/tool.rs:180), and runs run under `tokio::spawn` (agent.rs:459). With `send`, `Lua: Send + Sync`, callbacks and userdata must be `Send`, and async futures must be Send (`MaybeSend`). Without `send`, the Lua VM would need its own thread with a current-thread runtime and a `LocalSet`, plus a channel bridge. That is more code and gains nothing here.
- Running tool calls in parallel from Lua: the host provides `parallel{ f1, f2, ... }` (or `parallel(list)`) as an async function. It:
  1. turns each Lua function into `lua.create_thread(f)?.into_async::<Value>(())?`,
  2. awaits them with `futures_util::future::try_join_all` (or `join_all` to collect per-item errors),
  3. returns a Lua array in input order.

  Each child coroutine yields on its own tool awaits, so the MCP and built-in calls run at the same time on the same VM. The VM still runs only one thread at a time, which is fine because the work is I/O-bound. mlua's own tests drive several `into_async` threads at once (tests/async.rs), and 0.12.0 added "thread resolution for implicit async threads". **Prototype this nesting (an AsyncThread awaited inside an async callback of the same Lua) early.** A fallback with the same result: tool calls return a handle userdata, and an async `await_all(handles)` joins the Rust futures directly without nested Lua threads. Pi uses `Promise.all`, so `parallel{}` / `await_all` covers the same need.

- Time limits need two layers:
  1. wrap the whole run in `tokio::time::timeout` and `ToolCtx::cancel`. Dropping the `AsyncThread` future drops the pending tool futures.
  2. a Luau interrupt that checks an `Instant` deadline and returns `Err(RuntimeError("time limit"))`, and otherwise returns `VmState::Yield` every N ticks, so a tight Lua loop cannot block a tokio worker. Without the interrupt, `while true do end` never reaches an await point and the timeout never fires.

### JSON <-> Lua

- `features = ["serde"]` (0.12 renamed it from `serialize`, which is kept as an alias):
  - `LuaSerdeExt`: `lua.to_value(&serde_json::Value)` and `lua.from_value::<serde_json::Value>(v)`.
  - `lua.null()` is the JSON null sentinel, and `lua.array_metatable()` marks empty arrays. Since 0.12.0, "tables with the array metatable are always encoded as arrays".
  - Use `DeserializeOptions`/`SerializeOptions` (`deny_unsupported_types(false)`, `sort_keys`) for function and userdata values in results.
  - Empty table vs `[]` vs `{}` is the usual trap. Document `null` and `array{}` helpers for the model.

### Setup sketch

```rust
// mlua = { version = "0.12.1", features = ["luau", "async", "send", "serde"] }
use mlua::{Lua, LuaSerdeExt, StdLib, LuaOptions, VmState};
let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())?;
lua.set_memory_limit(64 << 20)?;
let deadline = std::time::Instant::now() + limit;
lua.set_interrupt(move |_| if std::time::Instant::now() > deadline {
    Err(mlua::Error::runtime("codemode: time limit"))
} else { Ok(VmState::Yield) });       // or yield every Nth tick
let call = lua.create_async_function(move |lua, (server, tool, args): (String, String, mlua::Value)| {
    let mcp = mcp.clone();
    async move {
        let args: serde_json::Value = lua.from_value(args)?;
        let out = mcp.call(&server, &tool, args).await.map_err(mlua::Error::external)?;
        lua.to_value(&out)            // structured_content if present, else content
    }
})?;
// register globals (tools.<server>.<tool>, parallel, ...) BEFORE sandbox(true)
lua.sandbox(true)?;                   // freezes globals and libraries; script globals go to a proxy
let result: serde_json::Value = lua.from_value(lua.load(script).set_name("codemode").eval_async().await?)?;
```

Use a new `Lua` per tool call: it is cheap, keeps state from leaking between calls, and gives each call its own memory limit. Pi keeps a persistent `state` object. If we want that, keep it in Rust as JSON and pass it in on each call.

### Alternatives

- **piccolo** 0.3.3 (MIT): a pure-Rust stackless Lua VM. It would remove the C++ build, but it has had **no release since 2024-06**, its standard library is incomplete, and it is slower. Not ready for production.
- **rhai** 1.26.1 (MIT/Apache, pure Rust): good sandboxing and operation limits, but it has **no async/await**. Each tool call would block or need a manual promise system. Models also know Lua/Luau syntax much better than Rhai. Not a good fit.
- (JS path, for completeness: `rquickjs` or `boa`. We chose Lua, so these were not evaluated further.)

## 3. JSON Schema -> signature text

No Rust crate does this for Luau, and none of the TypeScript ones fit:

- `schematype` (https://github.com/joris-gallot/schematype) is a Node/napi library that wraps openapiv3. It does not fit an in-process Rust use.
- `typify` and `schemafy` go JSON Schema -> Rust types.
- `typescript-type-def` goes Rust types -> TS.
- Cloudflare's and FastMCP's code-mode use JS's `json-schema-to-typescript`.
- Pi's codemode-mcp shows a short schema signature: required fields marked `*`, optional fields in brackets (https://github.com/mitsuhiko/pi-codemode-mcp).

Recommendation: write the converter by hand over `serde_json::Value`. It is about 150–250 lines plus property tests (hegeltest is already in the workspace). Map:

- `string`/`number`/`integer`/`boolean`/`null` to `string`/`number`/`number`/`boolean`/`nil`
- `array` + `items` to `{T}`
- `object` + `properties` to `{ a: T, b: T? }`, where a field not in `required` gets `?`
- `enum` of strings to a union of string singletons (`"a" | "b"`)
- `anyOf`/`oneOf` to `A | B`
- `const` to a singleton
- `$ref` to `#/$defs/X`, emitted as a named `type X = ...` alias
- anything else, or too deep, to `any`
- `description` to a `--` comment

Emit:

```luau
-- Search issues. (server: github)
function github.search_issues(args: { query: string, state: ("open" | "closed")?, limit: number? }): SearchResult
```

Derive the return type from `output_schema` when the server sends one, and use `any` when it does not. Keep the existing `jsonschema` crate (workspace 0.58.2) to check arguments in Rust before sending. Luau ignores annotations at run time, so the signatures serve only as prompt text.

## 4. tau repo constraints

- **Licenses** (deny.toml allow-list): Apache-2.0, BSD-2/3, CDLA-Permissive-2.0, ISC, MIT, MIT-0, MPL-2.0, Unicode-3.0, Zlib.
  - rmcp (Apache-2.0), mlua, mlua-sys, luau0-src, and lua-src (MIT) all pass.
  - The likely transitive dependencies (process-wrap, sse-stream, indexmap, chrono, oauth2) are MIT/Apache. Run `cargo deny check licenses` after adding them.
  - Leave out `auth-client-credentials-jwt` (aws-lc-rs) unless it is needed.
  - `multiple-versions = "warn"`: rmcp uses reqwest 0.13, tokio-util 0.7, and base64 0.23, which are the same lines as the workspace. Good.
- **Sources**: crates.io only. Do not use rmcp's git main.
- **Native builds**:
  - mlua+luau compiles C++ from `luau0-src` through the `cc` crate.
  - The default dev shell is `pkgs.mkShell`, a stdenv with gcc and g++. So plain `cargo build` in `nix develop` works with no flake change and no pkg-config entry. This follows the dev-shell rule (plain cargo must work).
  - Do **not** pick the system-Lua (`pkg-config`) route.
  - Android shell (`mkShellNoCC` + cargo-ndk): cc-rs uses the NDK's clang++, which builds Luau. Checked once the plugin ended up in the phone build (through tau-ui): it builds, and `luau0-src` links `c++_shared` on Android, so the shell sets `CARGO_NDK_LINK_LIBCXX_SHARED` and cargo-ndk copies `libc++_shared.so` into jniLibs for the APK.
  - The `buildRustPackage` for tau-ui uses stdenv, which has a C++ compiler, so the Nix package build is fine too.
- **tokio**: the workspace enables `macros, rt, sync, time`. rmcp's child-process transport adds `process` and `io-util`, which is fine for a plugin crate.
- **Send**: tau's tools need Send futures, so use mlua's `send` feature (see above).
- Per decision 0017 ("plugins bring their UI"), the plugin crate (crates/plugins/tau-mcp or codemode) has to ship its own UI pieces. That does not affect the library choice.

## Sources

- https://crates.io/crates/rmcp, https://github.com/modelcontextprotocol/rust-sdk (README, crates/rmcp/Cargo.toml, src/model.rs, src/service.rs, examples/clients/src/{git_stdio,streamable_http}.rs)
- https://docs.rs/rmcp
- https://github.com/modelcontextprotocol/rust-sdk/issues/857, https://github.com/modelcontextprotocol/rust-sdk/pull/858 (progress-aware timeouts)
- https://crates.io/crates/rust-mcp-sdk, https://crates.io/crates/tower-mcp
- https://crates.io/crates/mlua, https://github.com/mlua-rs/mlua (README, Cargo.toml, mlua-sys/Cargo.toml, CHANGELOG.md, tests/async.rs)
- https://docs.rs/mlua/0.12.1/mlua/struct.Lua.html, https://docs.rs/mlua/0.12.1/src/mlua/state.rs.html
- https://luau.org/sandbox
- https://crates.io/crates/luau0-src, https://crates.io/crates/piccolo, https://crates.io/crates/rhai
- https://github.com/joris-gallot/schematype, https://docs.rs/typify, https://github.com/mitsuhiko/pi-codemode-mcp, https://gofastmcp.com/v3/servers/transforms/code-mode
- tau: Cargo.toml, deny.toml, flake.nix, crates/tau-agent/src/tool.rs
