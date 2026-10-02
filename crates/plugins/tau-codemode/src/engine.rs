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
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use futures_util::future::join_all;
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
    value::{display, from_lua, to_lua},
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
        Ok(items) => {
            state.items.lock().expect("not poisoned").extend(items);
            None
        }
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
        let text = display(lua, &value).map_err(|error| {
            Failure::Error(format!("The script's return value: {error}"))
        })?;
        items.push(Item::Text(text));
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
    calls: Mutex<Calls>,
    store: Mutex<Store>,
    usage: Mutex<Usage>,
    stop: AtomicU8,
    exit: Notify,
    cancel: CancellationToken,
    deadline: Option<Instant>,
    ticks: AtomicU64,
    threads: Mutex<HashMap<usize, Thread>>,
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
            calls: Mutex::default(),
            store: Mutex::new(Store::new(store)),
            usage: Mutex::default(),
            stop: AtomicU8::new(RUNNING),
            exit: Notify::new(),
            cancel,
            deadline,
            ticks: AtomicU64::new(0),
            threads: Mutex::default(),
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
            Item::Text(text) => text.len(),
            Item::Image(image) => image.data.len(),
        };
        let total = self.output_bytes.fetch_add(size, Ordering::SeqCst) + size;
        if total > MAX_OUTPUT_BYTES {
            return Err(format!(
                "the script's output passed {} MiB",
                MAX_OUTPUT_BYTES >> 20
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
            });
            calls.started.push(Instant::now());
            calls.rows.len() - 1
        });
        let update = row
            .filter(|_| is_jev)
            .map(|row| JevUpdate::new(&calls.rows[row], after));
        drop(calls);
        let guard = CallGuard {
            state: self,
            row,
            id,
            jev_after: is_jev.then_some(after),
            done: false,
        };
        if let Some(update) = update {
            self.host.update(update.to_details());
        }
        guard
    }

    fn finish(
        &self,
        failure: Option<Failure>,
        wall: Duration,
        timeout_ms: Option<u64>,
    ) -> Outcome {
        let failure = failure.map(|failure| match failure {
            Failure::TimedOut { .. } => Failure::TimedOut {
                timeout_ms: timeout_ms.unwrap_or(0),
            },
            other => other,
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
    ) {
        self.record(status, error, cost);
    }

    fn record(
        &mut self,
        status: CallStatus,
        error: Option<&str>,
        cost: Option<f64>,
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
        let update = self.jev_after.map(|after| JevUpdate::new(row, after));
        drop(calls);
        if let Some(update) = update {
            self.state.host.update(update.to_details());
        }
    }
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.record(CallStatus::Cancelled, None, None);
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
                match display(lua, &value)
                    .and_then(|text| s.push(Item::Text(text)))
                {
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
                Ok(value) => {
                    guard.end(CallStatus::Ok, None, None);
                    Ok(ok([to_lua(&lua, &value)?]))
                }
                Err(error) => {
                    guard.end(CallStatus::Error, Some(&error), None);
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
                    guard.end(CallStatus::Ok, None, Some(cost));
                    Ok(ok([to_lua(&lua, &value)?]))
                }
                Err((error, cost)) => {
                    let text = error.to_string();
                    guard.end(CallStatus::Error, Some(&text), cost);
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
