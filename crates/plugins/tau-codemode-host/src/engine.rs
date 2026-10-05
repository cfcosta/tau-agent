//! Runs one script: a fresh Luau VM, its globals, and the loop that
//! drives it to the end, a timeout or a cancel.
//!
//! - **The VM**: every safe library, the globals, then `sandbox(true)`,
//!   with a 256 MiB memory limit.
//! - **The interrupt** fails the script once the deadline passes, the
//!   run is cancelled or `exit()` ran, and every [`YIELD_EVERY`] ticks
//!   yields the threads the engine runs (the script's and those of
//!   `parallel`), so a loop with no call in it neither holds a tokio
//!   worker nor outlives a cancel. A coroutine the script made is never
//!   yielded: its `resume` would see the yield as its own.
//! - **Globals that reach out** are Rust functions that return
//!   `(true, ...)` or `(false, message)`; a small Luau prelude turns the
//!   second into a Lua error with a string message, so `pcall` gets the
//!   message and not a userdata.
//! - **At the end** the engine resets its suspended threads and collects
//!   garbage, which drops the futures of calls still running: they are
//!   cancelled, and their rows say so.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        Mutex,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::{StreamExt, future::join_all, stream};
use mlua::{
    Function,
    Lua,
    LuaOptions,
    LuaSerdeExt,
    MultiValue,
    StdLib,
    Table,
    Thread,
    Value as LuaValue,
    VmState,
    chunk::ChunkMode,
};
use serde_json::{Value, json};
use tau_ai::message::Usage;
use tau_jev::Jev;
use tokio::sync::{Notify, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    host::{Host, Namespace, ToolCall, ToolEntry},
    image,
    jev,
    live::JevUpdate,
    modules::{self, Definition},
    options::Source,
    result::{
        CallRow,
        CallStatus,
        Failure,
        Item,
        MAX_ARGS_CHARS,
        MAX_CALL_ROWS,
        MAX_ERROR_CHARS,
        Outcome,
        preview,
    },
    search::{self, Index},
    signature,
    store::{Snapshot, Store},
    value::{display, from_lua, item, to_lua},
};

/// The VM's memory limit.
pub const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

/// Interrupt ticks between yields of the engine's threads.
pub const YIELD_EVERY: u64 = 4096;

/// Interrupt ticks between deadline checks.
const CLOCK_EVERY: u64 = 256;

/// The most output text a script may make, in bytes. The VM's memory
/// limit does not cover output, which lives outside it.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// The chunk name errors carry: `codemode:3: ...`.
const CHUNK: &str = "codemode";

const RUNNING: u8 = 0;
const TIMED_OUT: u8 = 1;
const CANCELLED: u8 = 2;
const EXITED: u8 = 3;

const PRELUDE: &str = r#"
local pack, unpack, error = table.pack, table.unpack, error
return function(f, level)
    return function(...)
        local r = pack(f(...))
        if r[1] then
            return unpack(r, 2, r.n)
        end
        error(r[2], level)
    end
end
"#;

/// One script to run.
pub struct Request {
    /// The codemode call's id: nested calls are `<call_id>/<n>`.
    pub call_id: String,
    pub source: Source,
    /// The store as the run's records fold it.
    pub store: Snapshot,
    pub cancel: CancellationToken,
}

/// Runs `request` against `host` to its end.
pub async fn run(host: Arc<dyn Host>, request: Request) -> Outcome {
    let started = Instant::now();
    let deadline = request
        .source
        .options
        .timeout_ms
        .map(|ms| started + Duration::from_millis(ms));
    let state = Arc::new(State::new(
        host,
        request.call_id,
        request.store,
        request.cancel,
        deadline,
    ));
    let failure = match drive(&state, &request.source.code).await {
        Ok(items) => items
            .into_iter()
            .find_map(|item| state.push(item).err().map(Failure::Error)),
        Err(failure) => Some(failure),
    };
    state.finish(
        failure,
        started.elapsed(),
        request.source.options.timeout_ms,
    )
}

/// Runs the script; `Ok` holds the items its return values make.
async fn drive(state: &Arc<State>, code: &str) -> Result<Vec<Item>, Failure> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
        .map_err(|error| Failure::Sandbox(error.to_string()))?;
    let setup = (|| -> mlua::Result<Thread> {
        install(&lua, state)?;
        lua.sandbox(true)?;
        lua.set_memory_limit(MEMORY_LIMIT)?;
        set_interrupt(&lua, state);
        let function = lua
            .load(code)
            .set_name(format!("={CHUNK}"))
            .set_mode(ChunkMode::Text)
            .into_function()?;
        let thread = lua.create_thread(function)?;
        state.own(&thread);
        Ok(thread)
    })();
    let thread = match setup {
        Ok(thread) => thread,
        Err(error @ mlua::Error::SyntaxError { .. }) => {
            return Err(Failure::Error(error_text(&error)));
        }
        Err(error) => return Err(Failure::Sandbox(error_text(&error))),
    };
    let ended = {
        let script = match thread.clone().into_async::<MultiValue>(()) {
            Ok(script) => script,
            Err(error) => return Err(Failure::Sandbox(error_text(&error))),
        };
        let deadline = state.deadline;
        let timer = async move {
            match deadline {
                Some(at) => tokio::time::sleep_until(at.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            result = script => Ended::Finished(result),
            () = state.exit.notified() => Ended::Exited,
            () = state.cancel.cancelled() => Ended::Cancelled,
            () = timer => Ended::TimedOut,
        }
    };
    if matches!(ended, Ended::Cancelled) {
        state.stop.store(CANCELLED, Ordering::SeqCst);
    }
    if matches!(ended, Ended::TimedOut) {
        state.stop.store(TIMED_OUT, Ordering::SeqCst);
    }
    let outcome = match (state.stop.load(Ordering::SeqCst), ended) {
        (TIMED_OUT, _) => Err(Failure::TimedOut { timeout_ms: 0 }),
        (CANCELLED, _) => Err(Failure::Cancelled),
        (EXITED, _) | (_, Ended::Exited) => Ok(Vec::new()),
        (_, Ended::Finished(Ok(values))) => returned(&lua, values),
        (_, Ended::Finished(Err(error))) => {
            Err(Failure::Error(error_text(&error)))
        }
        (_, Ended::Cancelled | Ended::TimedOut) => unreachable!("stop is set"),
    };
    close(&lua, state, &thread);
    outcome
}

enum Ended {
    Finished(mlua::Result<MultiValue>),
    Exited,
    Cancelled,
    TimedOut,
}

/// The items `return a, b` appends, in order; `nil` appends nothing.
fn returned(lua: &Lua, values: MultiValue) -> Result<Vec<Item>, Failure> {
    let mut items = Vec::new();
    for value in values {
        if value.is_nil() {
            continue;
        }
        items.push(item(lua, &value).map_err(|error| {
            Failure::Error(format!("The script's return value: {error}"))
        })?);
    }
    Ok(items)
}

/// Resets the engine's suspended threads and collects garbage, so the
/// futures of unfinished calls drop now. They hold the VM, so without
/// this the VM and they would keep each other alive.
fn close(lua: &Lua, state: &State, main: &Thread) {
    state
        .stop
        .compare_exchange(RUNNING, EXITED, Ordering::SeqCst, Ordering::SeqCst)
        .ok();
    let threads: Vec<Thread> = state
        .threads
        .lock()
        .expect("not poisoned")
        .drain()
        .map(|(_, thread)| thread)
        .chain(std::iter::once(main.clone()))
        .collect();
    // Cached tables/functions can refer to require closures, so break that
    // Lua -> Rust -> Lua ownership path before collecting the VM.
    state.modules.lock().expect("not poisoned").clear();
    lua.remove_interrupt();
    if let Ok(noop) = lua.create_function(|_, ()| Ok(())) {
        for thread in &threads {
            let _ = thread.reset(noop.clone());
        }
    }
    drop(threads);
    let _ = lua.gc_collect();
    let _ = lua.gc_collect();
}

/// The text of a Lua error, without its traceback. A message with no
/// position gets the script line the traceback names, if any.
pub fn error_text(error: &mlua::Error) -> String {
    match error {
        mlua::Error::CallbackError { traceback, cause } => {
            with_line(error_text(cause), traceback)
        }
        mlua::Error::RuntimeError(text) => {
            let (message, traceback) = match text.find("\nstack traceback:") {
                Some(at) => (&text[..at], &text[at..]),
                None => (text.as_str(), ""),
            };
            with_line(message.to_owned(), traceback)
        }
        mlua::Error::SyntaxError { message, .. } => message.clone(),
        mlua::Error::MemoryError(_) => "not enough memory".into(),
        mlua::Error::WithContext { context, cause } => {
            format!("{context}: {}", error_text(cause))
        }
        other => other.to_string(),
    }
}

fn with_line(message: String, traceback: &str) -> String {
    let prefix = format!("{CHUNK}:");
    if message.starts_with(&prefix) {
        return message;
    }
    let line = traceback.lines().find_map(|line| {
        let rest = line.trim().strip_prefix(&prefix)?;
        let digits: String =
            rest.chars().take_while(char::is_ascii_digit).collect();
        (!digits.is_empty() && rest[digits.len()..].starts_with(':'))
            .then_some(digits)
    });
    match line {
        Some(line) => format!("{CHUNK}:{line}: {message}"),
        None => message,
    }
}

/// Everything a script's globals share.
struct State {
    host: Arc<dyn Host>,
    call_id: String,
    tools: Vec<ToolEntry>,
    by_name: HashMap<String, usize>,
    namespaces: Vec<Namespace>,
    index: Index,
    jev: Option<Arc<dyn Jev>>,
    jev_slots: Semaphore,
    sequential: tokio::sync::Mutex<()>,
    items: Mutex<Vec<Item>>,
    output_bytes: AtomicUsize,
    output_byte_limit: usize,
    output_exceeded: AtomicBool,
    calls: Mutex<Calls>,
    store: Mutex<Store>,
    usage: Mutex<Usage>,
    stop: AtomicU8,
    exit: Notify,
    cancel: CancellationToken,
    deadline: Option<Instant>,
    ticks: AtomicU64,
    threads: Mutex<HashMap<usize, Thread>>,
    modules: Mutex<HashMap<String, LuaValue>>,
    module_aliases: Mutex<HashMap<String, String>>,
    module_load: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Calls {
    rows: Vec<CallRow>,
    started: Vec<Instant>,
    total: usize,
    tools: usize,
    jev: usize,
}

impl State {
    fn new(
        host: Arc<dyn Host>,
        call_id: String,
        store: Snapshot,
        cancel: CancellationToken,
        deadline: Option<Instant>,
    ) -> Self {
        let output_byte_limit = host.output_byte_limit();
        let mut tools = host.tools();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        tools.dedup_by(|a, b| a.name == b.name);
        let namespaces = host.namespaces();
        let by_name = tools
            .iter()
            .enumerate()
            .map(|(i, tool)| (tool.name.clone(), i))
            .collect();
        let index = Index::new(&tools, &namespaces);
        let jev = host.jev();
        Self {
            host,
            call_id,
            tools,
            by_name,
            namespaces,
            index,
            jev,
            jev_slots: Semaphore::new(jev::MAX_IN_FLIGHT),
            sequential: tokio::sync::Mutex::new(()),
            items: Mutex::default(),
            output_bytes: AtomicUsize::new(0),
            output_byte_limit,
            output_exceeded: AtomicBool::new(false),
            calls: Mutex::default(),
            store: Mutex::new(Store::new(store)),
            usage: Mutex::default(),
            stop: AtomicU8::new(RUNNING),
            exit: Notify::new(),
            cancel,
            deadline,
            ticks: AtomicU64::new(0),
            threads: Mutex::default(),
            modules: Mutex::default(),
            module_aliases: Mutex::default(),
            module_load: tokio::sync::Mutex::new(()),
        }
    }

    fn own(&self, thread: &Thread) {
        self.threads
            .lock()
            .expect("not poisoned")
            .insert(thread.to_pointer() as usize, thread.clone());
    }

    fn disown(&self, thread: &Thread) {
        self.threads
            .lock()
            .expect("not poisoned")
            .remove(&(thread.to_pointer() as usize));
    }

    fn owns_current(&self, lua: &Lua) -> bool {
        let current = lua.current_thread().to_pointer() as usize;
        self.threads
            .lock()
            .expect("not poisoned")
            .contains_key(&current)
    }

    fn push(&self, item: Item) -> Result<(), String> {
        let size = match &item {
            Item::Text(text) | Item::Json(text) => text.len(),
            Item::Image(image) => image.data.len(),
        };
        let total = self.output_bytes.fetch_add(size, Ordering::SeqCst) + size;
        if total > self.output_byte_limit {
            self.output_exceeded.store(true, Ordering::SeqCst);
            return Err(format!(
                "the script's output passed {} bytes",
                self.output_byte_limit
            ));
        }
        self.items.lock().expect("not poisoned").push(item);
        Ok(())
    }

    /// Starts a row; `jev` rows have their own numbering. A Jev row is
    /// reported to the host as it starts and as it ends ([`JevUpdate`]):
    /// its request makes no run events, unlike a tool call.
    fn begin(&self, name: &str, args: &Value, is_jev: bool) -> CallGuard<'_> {
        let mut calls = self.calls.lock().expect("not poisoned");
        calls.total += 1;
        let after = calls.tools;
        let id = if is_jev {
            calls.jev += 1;
            format!("{}/jev/{}", self.call_id, calls.jev)
        } else {
            calls.tools += 1;
            format!("{}/{}", self.call_id, calls.tools)
        };
        let row = (calls.rows.len() < MAX_CALL_ROWS).then(|| {
            calls.rows.push(CallRow {
                id: id.clone(),
                name: name.to_owned(),
                args: preview(&args.to_string(), MAX_ARGS_CHARS),
                status: CallStatus::Running,
                ms: 0,
                error: None,
                cost: None,
                usage_uncertain: false,
            });
            calls.started.push(Instant::now());
            calls.rows.len() - 1
        });
        let update = row.and_then(|row| {
            if is_jev {
                Some(JevUpdate::new(&calls.rows[row], after).to_details())
            } else if name == "infer" {
                Some(
                    crate::live::InferUpdate::new(&calls.rows[row])
                        .to_details(),
                )
            } else {
                None
            }
        });
        drop(calls);
        let guard = CallGuard {
            state: self,
            row,
            id,
            jev_after: is_jev.then_some(after),
            done: false,
        };
        if let Some(update) = update {
            self.host.update(update);
        }
        guard
    }

    fn finish(
        &self,
        failure: Option<Failure>,
        wall: Duration,
        timeout_ms: Option<u64>,
    ) -> Outcome {
        let failure = failure
            .map(|failure| match failure {
                Failure::TimedOut { .. } => Failure::TimedOut {
                    timeout_ms: timeout_ms.unwrap_or(0),
                },
                other => other,
            })
            .or_else(|| {
                self.output_exceeded.load(Ordering::SeqCst).then(|| {
                    Failure::Error(format!(
                        "the script's output passed {} bytes",
                        self.output_byte_limit
                    ))
                })
            });
        let mut calls =
            std::mem::take(&mut *self.calls.lock().expect("not poisoned"));
        for (row, started) in calls.rows.iter_mut().zip(&calls.started) {
            if row.status == CallStatus::Running {
                row.status = CallStatus::Cancelled;
                row.ms = started.elapsed().as_millis() as u64;
            }
        }
        let store = failure
            .is_none()
            .then(|| self.store.lock().expect("not poisoned").writes().clone());
        Outcome {
            failure,
            wall,
            items: std::mem::take(
                &mut *self.items.lock().expect("not poisoned"),
            ),
            calls: calls.rows,
            calls_total: calls.total,
            store,
            usage: self.usage.lock().expect("not poisoned").clone(),
        }
    }
}

/// A started row. Dropped unfinished, it is cancelled.
struct CallGuard<'a> {
    state: &'a State,
    row: Option<usize>,
    id: String,
    /// For a Jev row, the tool calls started before it: its row is
    /// reported to the host when it ends.
    jev_after: Option<usize>,
    done: bool,
}

impl CallGuard<'_> {
    fn end(
        mut self,
        status: CallStatus,
        error: Option<&str>,
        cost: Option<f64>,
        usage_uncertain: bool,
    ) {
        self.record(status, error, cost, usage_uncertain);
    }

    fn record(
        &mut self,
        status: CallStatus,
        error: Option<&str>,
        cost: Option<f64>,
        usage_uncertain: bool,
    ) {
        self.done = true;
        let Some(row) = self.row else {
            return;
        };
        let mut calls = self.state.calls.lock().expect("not poisoned");
        let ms = calls.started[row].elapsed().as_millis() as u64;
        let row = &mut calls.rows[row];
        row.status = status;
        row.ms = ms;
        row.error = error.map(|e| preview(e, MAX_ERROR_CHARS));
        row.cost = cost;
        row.usage_uncertain = usage_uncertain;
        let update = self
            .jev_after
            .map(|after| JevUpdate::new(row, after).to_details())
            .or_else(|| {
                (row.name == "infer")
                    .then(|| crate::live::InferUpdate::new(row).to_details())
            });
        drop(calls);
        if let Some(update) = update {
            self.state.host.update(update);
        }
    }
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.record(CallStatus::Cancelled, None, None, false);
        }
    }
}

fn set_interrupt(lua: &Lua, state: &Arc<State>) {
    let state = Arc::clone(state);
    lua.set_interrupt(move |lua| {
        let tick = state.ticks.fetch_add(1, Ordering::Relaxed);
        match state.stop.load(Ordering::Relaxed) {
            RUNNING => {}
            TIMED_OUT => return Err(mlua::Error::runtime("Script timed out")),
            CANCELLED => return Err(mlua::Error::runtime("Script cancelled")),
            _ => return Err(mlua::Error::runtime("exit")),
        }
        if state.cancel.is_cancelled() {
            state.stop.store(CANCELLED, Ordering::SeqCst);
            return Err(mlua::Error::runtime("Script cancelled"));
        }
        if tick.is_multiple_of(CLOCK_EVERY)
            && state.deadline.is_some_and(|at| Instant::now() >= at)
        {
            state.stop.store(TIMED_OUT, Ordering::SeqCst);
            return Err(mlua::Error::runtime("Script timed out"));
        }
        if tick % YIELD_EVERY == YIELD_EVERY - 1 && state.owns_current(lua) {
            return Ok(VmState::Yield);
        }
        Ok(VmState::Continue)
    });
}

/// `(true, values...)`.
fn ok(values: impl IntoIterator<Item = LuaValue>) -> MultiValue {
    let mut out = MultiValue::new();
    out.push_back(LuaValue::Boolean(true));
    out.extend(values);
    out
}

/// `(false, message)`.
fn fail(lua: &Lua, message: impl AsRef<str>) -> mlua::Result<MultiValue> {
    let mut out = MultiValue::new();
    out.push_back(LuaValue::Boolean(false));
    out.push_back(LuaValue::String(lua.create_string(message.as_ref())?));
    Ok(out)
}

/// Error levels for the prelude: `0` keeps the message as it is, `2`
/// adds the line of the script that called the global.
const AS_IS: u32 = 0;
const AT_CALLER: u32 = 2;

fn install(lua: &Lua, state: &Arc<State>) -> mlua::Result<()> {
    let lift: Function = lua.load(PRELUDE).set_name("=prelude").eval()?;
    let lifted = |f: Function, level: u32| -> mlua::Result<Function> {
        lift.call((f, level))
    };
    let globals = lua.globals();
    // mlua's `require` loads modules from files.
    globals.raw_remove("require")?;
    let s = Arc::clone(state);
    globals.set(
        "require",
        lifted(
            lua.create_async_function(move |lua, args: MultiValue| {
                let s = Arc::clone(&s);
                async move {
                    let (name, version) = match require_args(args) {
                        Ok(args) => args,
                        Err(message) => return fail(&lua, message),
                    };
                    match load_module(&lua, &s, &name, version.as_deref()).await
                    {
                        Ok(value) => Ok(ok([value])),
                        Err(message) => fail(&lua, message),
                    }
                }
            })?,
            AT_CALLER,
        )?,
    )?;

    // JSON shapes.
    let array_meta = lua.array_metatable();
    array_meta.set_readonly(true);
    let json = lua.create_table()?;
    json.set("null", lua.null())?;
    json.set(
        "encode",
        lifted(
            lua.create_function(
                |lua, value: LuaValue| match crate::json::encode(lua, &value) {
                    Ok(text) => {
                        Ok(ok([LuaValue::String(lua.create_string(text)?)]))
                    }
                    Err(error) => fail(lua, format!("json.encode(): {error}")),
                },
            )?,
            AT_CALLER,
        )?,
    )?;
    json.set(
        "decode",
        lifted(
            lua.create_function(|lua, text: LuaValue| {
                let LuaValue::String(text) = text else {
                    return fail(
                        lua,
                        "json.decode(): the text must be a string",
                    );
                };
                match crate::json::decode(&text.as_bytes()) {
                    Ok(value) => Ok(ok([to_lua(lua, &value)?])),
                    Err(error) => fail(lua, format!("json.decode(): {error}")),
                }
            })?,
            AT_CALLER,
        )?,
    )?;
    globals.set("json", json)?;
    globals.set(
        "array",
        lifted(
            lua.create_function(|lua, value: LuaValue| match value {
                LuaValue::Nil => {
                    let table = lua.create_table()?;
                    table.set_metatable(Some(lua.array_metatable()))?;
                    Ok(ok([LuaValue::Table(table)]))
                }
                LuaValue::Table(table) => {
                    table.set_metatable(Some(lua.array_metatable()))?;
                    Ok(ok([LuaValue::Table(table)]))
                }
                _ => fail(lua, "array() takes a table"),
            })?,
            AT_CALLER,
        )?,
    )?;

    // Output.
    let s = Arc::clone(state);
    globals.set(
        "text",
        lifted(
            lua.create_function(move |lua, value: LuaValue| {
                match item(lua, &value).and_then(|item| s.push(item)) {
                    Ok(()) => Ok(ok([])),
                    Err(error) => fail(lua, format!("text(): {error}")),
                }
            })?,
            AT_CALLER,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "print",
        lifted(
            lua.create_function(move |lua, values: MultiValue| {
                let mut parts = Vec::with_capacity(values.len());
                for value in &values {
                    let part = match value {
                        LuaValue::Nil => Ok("nil".to_owned()),
                        other => display(lua, other),
                    };
                    match part {
                        Ok(part) => parts.push(part),
                        Err(error) => {
                            return fail(lua, format!("print(): {error}"));
                        }
                    }
                }
                match s.push(Item::Text(parts.join("\t"))) {
                    Ok(()) => Ok(ok([])),
                    Err(error) => fail(lua, format!("print(): {error}")),
                }
            })?,
            AT_CALLER,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "image",
        lifted(
            lua.create_function(move |lua, value: LuaValue| {
                let parsed = from_lua(lua, &value)
                    .map_err(|error| format!("image(): {error}"))
                    .and_then(|json| image::parse(&json))
                    .and_then(|image| s.push(Item::Image(image)));
                match parsed {
                    Ok(()) => Ok(ok([])),
                    Err(error) => fail(lua, error),
                }
            })?,
            AT_CALLER,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "exit",
        lifted(
            lua.create_function(move |lua, ()| {
                s.stop
                    .compare_exchange(
                        RUNNING,
                        EXITED,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    )
                    .ok();
                s.exit.notify_one();
                fail(lua, "exit")
            })?,
            AS_IS,
        )?,
    )?;

    // The store.
    let s = Arc::clone(state);
    globals.set(
        "store",
        lifted(
            lua.create_function(
                move |lua, (key, value): (LuaValue, LuaValue)| {
                    let LuaValue::String(key) = key else {
                        return fail(lua, "store(): the key must be a string");
                    };
                    let key = key.to_string_lossy();
                    let value = match value {
                        LuaValue::Nil => None,
                        other => match from_lua(lua, &other) {
                            Ok(json) => Some(json),
                            Err(error) => {
                                return fail(lua, format!("store(): {error}"));
                            }
                        },
                    };
                    match s
                        .store
                        .lock()
                        .expect("not poisoned")
                        .store(&key, value)
                    {
                        Ok(()) => Ok(ok([])),
                        Err(error) => fail(lua, error),
                    }
                },
            )?,
            AT_CALLER,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "load",
        lifted(
            lua.create_function(move |lua, key: LuaValue| {
                let LuaValue::String(key) = key else {
                    return fail(lua, "load(): the key must be a string");
                };
                let value = s
                    .store
                    .lock()
                    .expect("not poisoned")
                    .load(&key.to_string_lossy())
                    .cloned();
                match value {
                    Some(value) => Ok(ok([to_lua(lua, &value)?])),
                    None => Ok(ok([LuaValue::Nil])),
                }
            })?,
            AT_CALLER,
        )?,
    )?;

    // Tools.
    let tools = lua.create_table()?;
    for (i, tool) in state.tools.iter().enumerate() {
        tools.set(
            tool.name.as_str(),
            lifted(tool_function(lua, state, i)?, AS_IS)?,
        )?;
    }
    globals.set("tools", tools)?;
    let s = Arc::clone(state);
    globals.set(
        "parallel",
        lifted(
            lua.create_async_function(move |lua, functions: MultiValue| {
                let s = Arc::clone(&s);
                async move {
                    let results =
                        match run_parallel(&lua, &s, functions).await? {
                            Ok(results) => results,
                            Err(message) => return fail(&lua, message),
                        };
                    let mut values = Vec::with_capacity(results.len());
                    for result in results {
                        match result {
                            Ok(mut returned) => {
                                values.push(
                                    returned
                                        .pop_front()
                                        .unwrap_or(LuaValue::Nil),
                                );
                            }
                            Err(error) => {
                                return fail(&lua, error_text(&error));
                            }
                        }
                    }
                    Ok(ok(values))
                }
            })?,
            AS_IS,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "parallel_settled",
        lifted(
            lua.create_async_function(move |lua, functions: MultiValue| {
                let s = Arc::clone(&s);
                async move {
                    let results =
                        match run_parallel(&lua, &s, functions).await? {
                            Ok(results) => results,
                            Err(message) => return fail(&lua, message),
                        };
                    let list = lua.create_table()?;
                    for result in results {
                        let entry = lua.create_table()?;
                        match result {
                            Ok(mut returned) => {
                                entry.set("ok", true)?;
                                entry.set(
                                    "value",
                                    returned
                                        .pop_front()
                                        .unwrap_or(LuaValue::Nil),
                                )?;
                            }
                            Err(error) => {
                                entry.set("ok", false)?;
                                entry.set("error", error_text(&error))?;
                            }
                        }
                        list.push(entry)?;
                    }
                    list.set_metatable(Some(lua.array_metatable()))?;
                    Ok(ok([LuaValue::Table(list)]))
                }
            })?,
            AS_IS,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "map",
        lifted(
            lua.create_async_function(move |lua, args: MultiValue| {
                let s = Arc::clone(&s);
                async move {
                    match run_map(&lua, &s, args).await? {
                        Ok(results) => Ok(ok([LuaValue::Table(results)])),
                        Err(message) => fail(&lua, message),
                    }
                }
            })?,
            AS_IS,
        )?,
    )?;

    // Discovery.
    let all = lua.create_table()?;
    for tool in &state.tools {
        all.push(entry_table(lua, tool)?)?;
    }
    all.set_metatable(Some(lua.array_metatable()))?;
    globals.set("ALL_TOOLS", all)?;
    let s = Arc::clone(state);
    globals.set(
        "search_tools",
        lifted(
            lua.create_function(
                move |lua, (query, options): (LuaValue, LuaValue)| {
                    let LuaValue::String(query) = query else {
                        return fail(
                            lua,
                            "search_tools(): the query must be a string",
                        );
                    };
                    let (limit, namespace) = match search_options(lua, &options)
                    {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            return fail(
                                lua,
                                format!("search_tools(): {error}"),
                            );
                        }
                    };
                    let found = s.index.search(
                        &query.to_string_lossy(),
                        limit,
                        namespace.as_deref(),
                    );
                    let list = lua.create_table()?;
                    for i in found {
                        list.push(entry_table(lua, &s.tools[i])?)?;
                    }
                    list.set_metatable(Some(lua.array_metatable()))?;
                    Ok(ok([LuaValue::Table(list)]))
                },
            )?,
            AT_CALLER,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "describe_tool",
        lifted(
            lua.create_function(move |lua, name: LuaValue| {
                let LuaValue::String(name) = name else {
                    return fail(
                        lua,
                        "describe_tool(): the name must be a string",
                    );
                };
                let Some(&i) = s.by_name.get(&name.to_string_lossy()) else {
                    return Ok(ok([LuaValue::Nil]));
                };
                Ok(ok([LuaValue::String(
                    lua.create_string(signature::describe(&s.tools[i]))?,
                )]))
            })?,
            AT_CALLER,
        )?,
    )?;
    let s = Arc::clone(state);
    globals.set(
        "describe_namespace",
        lifted(
            lua.create_function(move |lua, name: LuaValue| {
                let LuaValue::String(name) = name else {
                    return fail(
                        lua,
                        "describe_namespace(): the name must be a string",
                    );
                };
                let name = name.to_string_lossy();
                match namespace(&s, &name) {
                    Some(value) => Ok(ok([to_lua(lua, &value)?])),
                    None => Ok(ok([LuaValue::Nil])),
                }
            })?,
            AT_CALLER,
        )?,
    )?;

    // Jev.
    if state.jev.is_some() {
        let table = lua.create_table()?;
        for function in ["noul", "choice", "score", "ask"] {
            table.set(
                function,
                lifted(jev_function(lua, state, function)?, AS_IS)?,
            )?;
        }
        globals.set("jev", table)?;
    }
    Ok(())
}

fn entry_table(lua: &Lua, tool: &ToolEntry) -> mlua::Result<Table> {
    let entry = lua.create_table()?;
    entry.set("name", tool.name.as_str())?;
    entry.set("description", tool.description.as_str())?;
    entry.set_readonly(true);
    Ok(entry)
}

fn namespace(state: &State, name: &str) -> Option<Value> {
    let found = state
        .namespaces
        .iter()
        .find(|ns| search::same_namespace(&ns.name, name));
    let space = match found {
        Some(ns) => ns.name.clone(),
        None => state
            .tools
            .iter()
            .filter_map(|t| t.namespace.as_ref())
            .find(|ns| search::same_namespace(ns, name))?
            .clone(),
    };
    let tools: Vec<&str> = state
        .tools
        .iter()
        .filter(|t| t.namespace.as_deref() == Some(space.as_str()))
        .map(|t| t.name.as_str())
        .collect();
    Some(json!({
        "name": space,
        "description": found.and_then(|ns| ns.description.clone()),
        "instructions": found.and_then(|ns| ns.instructions.clone()),
        "tools": tools,
    }))
}

fn search_options(
    lua: &Lua,
    options: &LuaValue,
) -> Result<(usize, Option<String>), String> {
    let options = match options {
        LuaValue::Nil => return Ok((search::DEFAULT_LIMIT, None)),
        other => from_lua(lua, other)?,
    };
    let Value::Object(map) = options else {
        return Err("options must be a table".into());
    };
    let limit = match map.get("limit") {
        None | Some(Value::Null) => search::DEFAULT_LIMIT,
        Some(value) => value
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("`limit` must be a positive integer")?
            as usize,
    };
    let namespace = match map.get("namespace") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(name.clone()),
        Some(_) => return Err("`namespace` must be a string".into()),
    };
    Ok((limit, namespace))
}

/// `tools.<name>`.
fn tool_function(
    lua: &Lua,
    state: &Arc<State>,
    i: usize,
) -> mlua::Result<Function> {
    let state = Arc::clone(state);
    lua.create_async_function(move |lua, args: LuaValue| {
        let state = Arc::clone(&state);
        async move {
            let tool = &state.tools[i];
            let args = match args {
                LuaValue::Nil => json!({}),
                other => match from_lua(&lua, &other) {
                    Ok(json) => json,
                    Err(error) => {
                        return fail(
                            &lua,
                            format!(
                                "tools.{}: the arguments: {error}",
                                tool.name
                            ),
                        );
                    }
                },
            };
            if let Some(message) = stopped(&state) {
                return fail(&lua, message);
            }
            let _turn = if tool.sequential {
                Some(state.sequential.lock().await)
            } else {
                None
            };
            let guard = state.begin(&tool.name, &args, false);
            let call = ToolCall {
                id: guard.id.clone(),
                name: tool.name.clone(),
                args,
            };
            match state.host.call_tool(call).await {
                Ok(reply) => {
                    let status = if reply.error.is_some() {
                        CallStatus::Error
                    } else {
                        CallStatus::Ok
                    };
                    if let Some(usage) =
                        reply.usage.as_ref().filter(|_| tool.name == "infer")
                    {
                        *state.usage.lock().expect("not poisoned") += usage;
                    }
                    let cost = (tool.name == "infer")
                        .then_some(reply.usage.as_ref())
                        .flatten()
                        .map(|usage| usage.cost.total);
                    guard.end(
                        status,
                        reply.error.as_deref(),
                        cost,
                        tool.name == "infer"
                            && reply.usage_complete == Some(false),
                    );
                    Ok(ok([to_lua(&lua, &reply.value)?]))
                }
                Err(error) => {
                    guard.end(CallStatus::Error, Some(&error), None, false);
                    fail(&lua, error)
                }
            }
        }
    })
}

/// Why no new call may start, if the script is ending.
fn stopped(state: &State) -> Option<&'static str> {
    match state.stop.load(Ordering::SeqCst) {
        RUNNING => None,
        TIMED_OUT => Some("Script timed out"),
        CANCELLED => Some("Script cancelled"),
        _ => Some("exit"),
    }
}

fn require_args(args: MultiValue) -> Result<(String, Option<String>), String> {
    if args.is_empty() || args.len() > 2 {
        return Err(
            "require() expects a module name and optional version".into()
        );
    }
    let mut args = args.into_iter();
    let Some(LuaValue::String(name)) = args.next() else {
        return Err("require(): the name must be a string".into());
    };
    let name = name
        .to_str()
        .map_err(|_| "require(): invalid UTF-8 name")?
        .to_string();
    modules::validate_name(&name)
        .map_err(|error| format!("require(): {error}"))?;
    let version = match args.next() {
        None | Some(LuaValue::Nil) => None,
        Some(LuaValue::String(version)) => {
            let version = version
                .to_str()
                .map_err(|_| "require(): invalid UTF-8 version")?
                .to_string();
            if !modules::valid_version(&version) {
                return Err(
                    "require(): version must be 64 lowercase hex characters"
                        .into(),
                );
            }
            Some(version)
        }
        Some(_) => return Err("require(): the version must be a string".into()),
    };
    Ok((name, version))
}

/// Resolve the declared graph before evaluating any source. This catches
/// cycles across simultaneous imports without waiting on another importer.
async fn visit_module(
    state: &State,
    name: &str,
    version: Option<&str>,
    path: &mut Vec<String>,
    definitions: &mut HashMap<String, Definition>,
    order: &mut Vec<String>,
) -> Result<String, String> {
    if let Some(message) = stopped(state) {
        return Err(message.into());
    }
    let definition = state.host.module(name, version).await?.ok_or_else(
        || match version {
            Some(version) => {
                format!("module {name}@{version} is not registered")
            }
            None => format!("module {name} is not registered"),
        },
    )?;
    definition.verify()?;
    if definition.name() != name
        || version.is_some_and(|v| v != definition.version())
    {
        return Err(format!("module {name} returned a mismatched version"));
    }
    let key = definition.version().to_owned();
    if path.contains(&key) {
        let mut chain = path.clone();
        chain.push(key);
        return Err(format!("module import cycle: {}", chain.join(" -> ")));
    }
    if definitions.contains_key(&key) {
        return Ok(key);
    }
    path.push(key.clone());
    for (child, pin) in definition.dependencies() {
        Box::pin(visit_module(
            state,
            child,
            Some(pin),
            path,
            definitions,
            order,
        ))
        .await?;
    }
    path.pop();
    order.push(key.clone());
    definitions.insert(key.clone(), definition);
    Ok(key)
}

async fn load_module(
    lua: &Lua,
    state: &Arc<State>,
    name: &str,
    version: Option<&str>,
) -> Result<LuaValue, String> {
    let _loading = state.module_load.lock().await;
    if version.is_none()
        && let Some(pin) = state
            .module_aliases
            .lock()
            .expect("not poisoned")
            .get(name)
            .cloned()
    {
        return Ok(state.modules.lock().expect("not poisoned")[&pin].clone());
    }
    let mut definitions = HashMap::new();
    let mut order = Vec::new();
    let key = visit_module(
        state,
        name,
        version,
        &mut Vec::new(),
        &mut definitions,
        &mut order,
    )
    .await?;
    for version in order {
        if state
            .modules
            .lock()
            .expect("not poisoned")
            .contains_key(&version)
        {
            continue;
        }
        let definition = &definitions[&version];
        let value = evaluate_module(lua, state, definition).await?;
        state
            .modules
            .lock()
            .expect("not poisoned")
            .insert(version, value);
    }
    if version.is_none() {
        state
            .module_aliases
            .lock()
            .expect("not poisoned")
            .insert(name.to_owned(), key.clone());
    }
    Ok(state.modules.lock().expect("not poisoned")[&key].clone())
}

async fn evaluate_module(
    lua: &Lua,
    state: &Arc<State>,
    definition: &Definition,
) -> Result<LuaValue, String> {
    if let Some(message) = stopped(state) {
        return Err(message.into());
    }
    let environment = lua.create_table().map_err(|e| e.to_string())?;
    let meta = lua.create_table().map_err(|e| e.to_string())?;
    let globals = lua.globals();
    let inherit = lua
        .create_function(move |_, (_environment, key): (Table, LuaValue)| {
            if let LuaValue::String(name) = &key
                && matches!(name.to_str()?.as_ref(), "getfenv" | "setfenv")
            {
                return Ok(LuaValue::Boolean(false));
            }
            globals.get::<LuaValue>(key)
        })
        .map_err(|e| e.to_string())?;
    // A module can delete its own fields. Keep introspection blocked in
    // the fallback too, so nil/rawset cannot recover the caller's require.
    meta.set("__index", inherit).map_err(|e| e.to_string())?;
    meta.set("__metatable", "locked")
        .map_err(|e| e.to_string())?;
    environment
        .set_metatable(Some(meta))
        .map_err(|e| e.to_string())?;
    environment
        .set("_G", environment.clone())
        .map_err(|e| e.to_string())?;
    // Environment introspection could recover the caller's unrestricted
    // require and bypass this definition's dependency pins.
    environment
        .set("getfenv", false)
        .map_err(|e| e.to_string())?;
    environment
        .set("setfenv", false)
        .map_err(|e| e.to_string())?;
    let pins = definition.dependencies().clone();
    let cache = Arc::clone(state);
    let dependency_require = lua
        .create_function(move |lua, args: MultiValue| {
            let (name, version) = match require_args(args) {
                Ok(args) => args,
                Err(message) => return fail(lua, message),
            };
            let Some(pin) = pins.get(&name) else {
                return fail(
                    lua,
                    format!("module dependency {name} is not declared"),
                );
            };
            if version.as_deref().is_some_and(|version| version != pin) {
                return fail(
                    lua,
                    format!("module dependency {name} version mismatch"),
                );
            }
            match cache
                .modules
                .lock()
                .expect("not poisoned")
                .get(pin)
                .cloned()
            {
                Some(value) => Ok(ok([value])),
                None => fail(
                    lua,
                    format!("module dependency {name}@{pin} is unavailable"),
                ),
            }
        })
        .map_err(|e| e.to_string())?;
    // Keep the same string-error contract as top-level require.
    let lift: Function = lua.load(PRELUDE).eval().map_err(|e| e.to_string())?;
    let dependency_require: Function = lift
        .call((dependency_require, AT_CALLER))
        .map_err(|e| e.to_string())?;
    environment
        .set("require", dependency_require)
        .map_err(|e| e.to_string())?;
    let function = lua
        .load(definition.source())
        .set_name(format!(
            "=module:{}@{}",
            definition.name(),
            definition.version()
        ))
        .set_mode(ChunkMode::Text)
        .set_environment(environment)
        .into_function()
        .map_err(|error| error_text(&error))?;
    let thread = lua.create_thread(function).map_err(|e| e.to_string())?;
    state.own(&thread);
    let result = thread
        .clone()
        .into_async::<MultiValue>(())
        .map_err(|error| error_text(&error))?
        .await;
    state.disown(&thread);
    let values = result.map_err(|error| error_text(&error))?;
    if values.len() != 1 {
        return Err(format!(
            "module {} must return exactly one value",
            definition.name()
        ));
    }
    let value = values.into_iter().next().expect("one value");
    if !matches!(value, LuaValue::Function(_) | LuaValue::Table(_)) {
        return Err(format!(
            "module {} must return a function or table",
            definition.name()
        ));
    }
    Ok(value)
}

/// `jev.<function>`.
fn jev_function(
    lua: &Lua,
    state: &Arc<State>,
    function: &'static str,
) -> mlua::Result<Function> {
    let state = Arc::clone(state);
    lua.create_async_function(move |lua, args: LuaValue| {
        let state = Arc::clone(&state);
        async move {
            let Some(client) = state.jev.clone() else {
                return fail(&lua, "Jev is not available in this run");
            };
            let args = match from_lua(&lua, &args) {
                Ok(json) => json,
                Err(error) => {
                    return fail(&lua, format!("jev.{function}: {error}"));
                }
            };
            let (request, shape) = match jev::request(function, &args) {
                Ok(parsed) => parsed,
                Err(error) => return fail(&lua, error),
            };
            if let Some(message) = stopped(&state) {
                return fail(&lua, message);
            }
            let Ok(_slot) = state.jev_slots.acquire().await else {
                return fail(&lua, "Jev is closed");
            };
            let guard = state.begin(&format!("jev.{function}"), &args, true);
            let answer = match client.ask(&request).await {
                Ok(response) => {
                    let usage = response.usage();
                    state.host.charge(&usage);
                    *state.usage.lock().expect("not poisoned") += &usage;
                    let cost = usage.cost.total;
                    jev::answer(&shape, &response)
                        .map(|value| (value, cost))
                        .map_err(|error| (error, Some(cost)))
                }
                Err(error) => Err((error, None)),
            };
            match answer {
                Ok((value, cost)) => {
                    guard.end(CallStatus::Ok, None, Some(cost), false);
                    Ok(ok([to_lua(&lua, &value)?]))
                }
                Err((error, cost)) => {
                    let text = error.to_string();
                    guard.end(CallStatus::Error, Some(&text), cost, false);
                    fail(&lua, text)
                }
            }
        }
    })
}

/// A value's type as Luau's `type()` names it.
fn type_name(value: &LuaValue) -> &'static str {
    match value {
        LuaValue::Integer(_) => "number",
        other => other.type_name(),
    }
}

/// Runs `functions` as threads at once and waits for all of them.
/// `Ok(Err(message))` when an argument is not a function.
async fn run_parallel(
    lua: &Lua,
    state: &Arc<State>,
    functions: MultiValue,
) -> mlua::Result<Result<Vec<mlua::Result<MultiValue>>, String>> {
    let mut threads = Vec::with_capacity(functions.len());
    for (i, value) in functions.into_iter().enumerate() {
        let LuaValue::Function(function) = value else {
            return Ok(Err(format!(
                "parallel: argument {} is a {}, not a function",
                i + 1,
                type_name(&value)
            )));
        };
        threads.push(lua.create_thread(function)?);
    }
    let mut running = Vec::with_capacity(threads.len());
    for thread in &threads {
        state.own(thread);
        running.push(thread.clone().into_async::<MultiValue>(())?);
    }
    let results = join_all(running).await;
    for thread in &threads {
        state.disown(thread);
    }
    Ok(Ok(results))
}

/// Maps a dense, marked array with at most `concurrency` running callbacks.
/// Each callback thread is created only when its buffered future is polled.
async fn run_map(
    lua: &Lua,
    state: &Arc<State>,
    args: MultiValue,
) -> mlua::Result<Result<Table, String>> {
    let mut args = args.into_iter();
    let Some(LuaValue::Table(items)) = args.next() else {
        return Ok(Err("map: items must be an array".into()));
    };
    if !items
        .metatable()
        .is_some_and(|meta| meta == lua.array_metatable())
    {
        return Ok(Err("map: items must be a marked array".into()));
    }
    let Some(LuaValue::Function(callback)) = args.next() else {
        return Ok(Err("map: fn must be a function".into()));
    };
    let concurrency = match args.next().unwrap_or(LuaValue::Nil) {
        LuaValue::Nil => 4,
        LuaValue::Integer(n) if (1..=32).contains(&n) => n as usize,
        LuaValue::Number(n)
            if n.fract() == 0.0 && (1.0..=32.0).contains(&n) =>
        {
            n as usize
        }
        _ => {
            return Ok(Err(
                "map: concurrency must be an integer from 1 to 32".into()
            ));
        }
    };
    if args.next().is_some() {
        return Ok(Err(
            "map: expected items, fn, and optional concurrency".into()
        ));
    }

    let mut count = 0;
    for pair in items.clone().pairs::<LuaValue, LuaValue>() {
        let (key, _) = pair?;
        count += 1;
        if count > 10_000 {
            return Ok(Err("map: items cannot exceed 10000 elements".into()));
        }
        let index = match key {
            LuaValue::Integer(n) => n,
            LuaValue::Number(n) if n.fract() == 0.0 => n as i64,
            _ => return Ok(Err("map: items must be a dense array".into())),
        };
        if !(1..=10_000).contains(&index) {
            return Ok(Err("map: items must be a dense array".into()));
        }
    }
    // `count` keys all within 1..=count proves there are no holes or
    // mixed keys; Lua tables cannot hold duplicate keys.
    let mut values = Vec::with_capacity(count);
    for index in 1..=count {
        let item = items.raw_get::<LuaValue>(index)?;
        if item.is_nil() {
            return Ok(Err("map: items must be a dense array".into()));
        }
        values.push(item);
    }

    let callbacks =
        stream::iter(values.into_iter().enumerate().map(|(index, item)| {
            let callback = callback.clone();
            let state = Arc::clone(state);
            async move {
                let thread = lua.create_thread(callback)?;
                state.own(&thread);
                let result = thread
                    .clone()
                    .into_async::<MultiValue>((item, index + 1))?
                    .await;
                state.disown(&thread);
                Ok::<_, mlua::Error>((index, result))
            }
        }))
        .buffer_unordered(concurrency);
    futures_util::pin_mut!(callbacks);
    let mut settled = vec![None; count];
    while let Some(result) = callbacks.next().await {
        let (index, result) = result?;
        settled[index] = Some(result);
    }
    let list = lua.create_table_with_capacity(count, 0)?;
    for result in settled.into_iter().flatten() {
        let entry = lua.create_table()?;
        match result {
            Ok(mut returned) => {
                entry.set("ok", true)?;
                let value = match returned.pop_front() {
                    None | Some(LuaValue::Nil) => lua.null(),
                    Some(value) => value,
                };
                entry.set("value", value)?;
            }
            Err(error) => {
                entry.set("ok", false)?;
                entry.set("error", error_text(&error))?;
            }
        }
        list.push(entry)?;
    }
    list.set_metatable(Some(lua.array_metatable()))?;
    Ok(Ok(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_without_a_position_takes_the_traceback_line() {
        let traceback = "stack traceback:\n\t[C]: in function 'error'\n\tprelude:9: in function <prelude:4>\n\tcodemode:3: in function <codemode:1>";
        assert_eq!(with_line("boom".into(), traceback), "codemode:3: boom");
        assert_eq!(
            with_line("codemode:1: boom".into(), traceback),
            "codemode:1: boom"
        );
        assert_eq!(with_line("boom".into(), ""), "boom");
    }
}
