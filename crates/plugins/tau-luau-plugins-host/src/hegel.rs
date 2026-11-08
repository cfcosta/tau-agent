//! Property tests' engine (ADR 0049): hegel's, through its C ABI, which
//! every language's frontend drives. The engine offers a test case at a
//! time; the plugin's Luau draws from it through the `hegel` tool
//! ([`Case::call`]), and the host says how the case ended.
//!
//! Every `unsafe` of the plugins' host is here. Each handle is owned by
//! one value and freed when it drops; a call reads the engine's error
//! from the calling thread's context right after it.

use std::{
    ffi::{CStr, CString, c_char, c_void},
    ptr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hegel_c::{
    HegelCollection,
    HegelContext,
    HegelRun,
    HegelStateMachine,
    HegelStringGenerator,
    HegelTestCase,
    hegel_health_check_t,
    hegel_result_t,
    hegel_run_status_t,
    hegel_status_t,
};
use serde_json::{Value, json};

/// The largest integer a Luau number holds exactly: integers drawn with
/// no bound stay within it.
const EXACT: i64 = 1 << 53;

/// The most a case may ask of the engine. One that asks more is an
/// overrun, as when the engine's choices run out: a property that draws
/// without end then ends, and the VM's own bookkeeping of the calls
/// stays bounded.
const MOST_CALLS: u32 = 50_000;

thread_local! {
    /// The engine's error channel for calls made on this thread.
    static CONTEXT: Context = Context(hegel_c::hegel_context_new());
}

struct Context(*mut HegelContext);

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: made by hegel_context_new for this thread, freed once.
        let _ = unsafe { hegel_c::hegel_context_free(self.0) };
    }
}

fn with_context<R>(f: impl FnOnce(*mut HegelContext) -> R) -> R {
    CONTEXT.with(|context| f(context.0))
}

/// The engine's message for the last call on this thread that failed.
fn last_error() -> String {
    with_context(|ctx| {
        // SAFETY: ctx is this thread's live context; the string it gives
        // lives until the next call on it, and is copied at once.
        let text = unsafe { hegel_c::hegel_context_last_error(ctx) };
        if text.is_null() {
            return "hegel failed, and did not say why".to_owned();
        }
        unsafe { CStr::from_ptr(text) }
            .to_string_lossy()
            .into_owned()
    })
}

fn check(rc: hegel_result_t) -> Result<(), String> {
    if rc == hegel_result_t::HEGEL_OK {
        Ok(())
    } else {
        Err(last_error())
    }
}

fn text(value: &str) -> CString {
    CString::new(value.replace('\0', "\u{FFFD}")).expect("no NUL left")
}

/// How a property's run is set: the host's, never the plugin's.
#[derive(Debug, Clone)]
pub struct Settings {
    pub cases: u64,
    pub seed: u64,
}

/// The engine's lines for a run, gathered rather than printed.
type Lines = Mutex<Vec<String>>;

unsafe extern "C" fn gather(
    user_data: *mut c_void,
    line: *const c_char,
    len: usize,
) {
    // SAFETY: user_data is the `Lines` a `Run` keeps alive until the
    // engine's worker is joined; line holds len bytes for this call.
    let lines = unsafe { &*user_data.cast::<Lines>() };
    let bytes = unsafe { std::slice::from_raw_parts(line.cast::<u8>(), len) };
    lines
        .lock()
        .expect("not poisoned")
        .push(String::from_utf8_lossy(bytes).trim_end().to_owned());
}

/// One property's run: the engine exploring, a case at a time.
pub struct Run {
    raw: *mut HegelRun,
    lines: *mut Lines,
}

// SAFETY: the engine allows a run handle on any thread, one at a time;
// a `Run` is used by one task, its calls one after another.
unsafe impl Send for Run {}

/// What one failure of a property came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub origin: String,
    pub blob: Option<String>,
    pub caveat: Option<String>,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Passed,
    Failed(Vec<Failure>),
    /// The run itself failed: a health check, or the engine's own error.
    Broken(String),
}

impl Run {
    /// Starts a run: exploring, or replaying `blob`.
    pub fn start(
        settings: &Settings,
        blob: Option<&str>,
    ) -> Result<Self, String> {
        let mut raw_settings = ptr::null_mut();
        with_context(|ctx| unsafe {
            // SAFETY: ctx is live; each out-parameter is valid.
            check(hegel_c::hegel_settings_new(ctx, &mut raw_settings))?;
            let configured = (|| {
                check(hegel_c::hegel_settings_set_test_cases(
                    ctx,
                    raw_settings,
                    settings.cases,
                ))?;
                check(hegel_c::hegel_settings_set_seed(
                    ctx,
                    raw_settings,
                    settings.seed,
                    true,
                ))?;
                check(hegel_c::hegel_settings_set_derandomize(
                    ctx,
                    raw_settings,
                    true,
                ))?;
                let none = text("");
                check(hegel_c::hegel_settings_set_database(
                    ctx,
                    raw_settings,
                    none.as_ptr(),
                ))?;
                check(hegel_c::hegel_settings_set_suppress_health_check(
                    ctx,
                    raw_settings,
                    hegel_health_check_t::HEGEL_HC_TOO_SLOW as u32,
                ))?;
                check(hegel_c::hegel_settings_set_print_blob(
                    ctx,
                    raw_settings,
                    false,
                ))
            })();
            if let Err(error) = configured {
                let _ = hegel_c::hegel_settings_free(ctx, raw_settings);
                return Err(error);
            }
            let lines: *mut Lines = Box::into_raw(Box::default());
            let mut raw = ptr::null_mut();
            let rc = match blob {
                Some(blob) => {
                    let blob = text(blob);
                    hegel_c::hegel_run_start_blob(
                        ctx,
                        raw_settings,
                        blob.as_ptr(),
                        Some(gather),
                        lines.cast(),
                        &mut raw,
                    )
                }
                None => hegel_c::hegel_run_start(
                    ctx,
                    raw_settings,
                    Some(gather),
                    lines.cast(),
                    &mut raw,
                ),
            };
            let started = check(rc);
            let _ = hegel_c::hegel_settings_free(ctx, raw_settings);
            // Made before the check, so a run that did not start frees
            // its lines too (the engine accepts a null run).
            let run = Self { raw, lines };
            started.map(|()| run)
        })
    }

    /// The next case the engine wants run, or `None` once it is done.
    /// It blocks while the engine works, so it is called off the
    /// runtime's threads.
    pub fn next_case(&mut self) -> Result<Option<Case>, String> {
        let mut raw = ptr::null_mut();
        // SAFETY: self.raw is a live run, used by this call alone.
        check(with_context(|ctx| unsafe {
            hegel_c::hegel_next_test_case(ctx, self.raw, &mut raw)
        }))?;
        Ok((!raw.is_null()).then(|| Case {
            raw,
            collections: Vec::new(),
            machines: Vec::new(),
            overran: false,
            done: false,
            calls: 0,
        }))
    }

    /// What the run came to, once [`Self::next_case`] gave `None`.
    pub fn verdict(&self) -> Result<Verdict, String> {
        with_context(|ctx| unsafe {
            // SAFETY: self.raw is a drained run; every string read is
            // copied before the result it belongs to is freed.
            let mut result = ptr::null_mut();
            check(hegel_c::hegel_run_result(ctx, self.raw, &mut result))?;
            let read = (|| {
                let mut status = hegel_run_status_t::HEGEL_RUN_STATUS_PASSED;
                check(hegel_c::hegel_run_result_status(
                    ctx,
                    result,
                    &mut status,
                ))?;
                match status {
                    hegel_run_status_t::HEGEL_RUN_STATUS_PASSED => {
                        Ok(Verdict::Passed)
                    }
                    hegel_run_status_t::HEGEL_RUN_STATUS_FAILED => {
                        let mut count = 0;
                        check(hegel_c::hegel_run_result_failure_count(
                            ctx, result, &mut count,
                        ))?;
                        let mut failures = Vec::new();
                        for index in 0..count {
                            let mut failure = ptr::null_mut();
                            check(hegel_c::hegel_run_result_failure(
                                ctx,
                                result,
                                index,
                                &mut failure,
                            ))?;
                            let string = |read: unsafe extern "C" fn(
                                *mut HegelContext,
                                *const hegel_c::HegelFailure,
                                *mut *const c_char,
                            )
                                -> hegel_result_t| {
                                let mut out = ptr::null();
                                let rc = read(ctx, failure, &mut out);
                                (rc == hegel_result_t::HEGEL_OK && !out.is_null()).then(
                                    || CStr::from_ptr(out).to_string_lossy().into_owned(),
                                )
                            };
                            failures.push(Failure {
                                origin: string(hegel_c::hegel_failure_origin)
                                    .unwrap_or_default(),
                                blob: string(
                                    hegel_c::hegel_failure_reproduction_blob,
                                ),
                                caveat: string(hegel_c::hegel_failure_caveat),
                            });
                            let _ = hegel_c::hegel_failure_free(ctx, failure);
                        }
                        Ok(Verdict::Failed(failures))
                    }
                    _ => {
                        let mut error = ptr::null();
                        check(hegel_c::hegel_run_result_error(
                            ctx, result, &mut error,
                        ))?;
                        Ok(Verdict::Broken(if error.is_null() {
                            "the run failed".to_owned()
                        } else {
                            CStr::from_ptr(error).to_string_lossy().into_owned()
                        }))
                    }
                }
            })();
            let _ = hegel_c::hegel_run_result_free(ctx, result);
            read
        })
    }

    /// The engine's lines so far.
    pub fn lines(&self) -> Vec<String> {
        // SAFETY: lines lives until this run drops.
        unsafe { &*self.lines }
            .lock()
            .expect("not poisoned")
            .clone()
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        with_context(|ctx| unsafe {
            // SAFETY: freeing the run stops and joins the engine's worker,
            // the only other user of `lines`, which is then freed once.
            let _ = hegel_c::hegel_run_free(ctx, self.raw);
            drop(Box::from_raw(self.lines));
        });
    }
}

/// How the plugin's Luau said a case ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Valid,
    /// It assumed something that did not hold.
    Invalid,
    /// It failed: this case is a counterexample.
    Interesting,
}

/// One case the engine offered, and what its draws opened.
pub struct Case {
    raw: *mut HegelTestCase,
    collections: Vec<*mut HegelCollection>,
    machines: Vec<*mut HegelStateMachine>,
    /// Whether a draw ran out of the engine's choices: the case is then
    /// inconclusive, whatever the Luau made of the error.
    overran: bool,
    done: bool,
    /// What it asked of the engine so far.
    calls: u32,
}

// SAFETY: the engine guards each handle with its own lock; a `Case` is
// behind a mutex, so one thread uses it at a time.
unsafe impl Send for Case {}

impl Case {
    /// Whether the engine wants this case's report kept: the replay it
    /// reports a failure from.
    pub fn should_capture(&self) -> bool {
        let mut capture = false;
        // SAFETY: self.raw is live.
        let _ = with_context(|ctx| unsafe {
            hegel_c::hegel_test_case_should_capture(ctx, self.raw, &mut capture)
        });
        capture
    }

    /// Whether a draw ran out of the engine's choices.
    pub fn overran(&self) -> bool {
        self.overran
    }

    /// Ends the case, `origin` naming a failure.
    pub fn complete(
        &mut self,
        ended: Ended,
        origin: &str,
    ) -> Result<(), String> {
        let (status, origin) = match ended {
            _ if self.overran => (hegel_status_t::HEGEL_STATUS_OVERRUN, None),
            Ended::Valid => (hegel_status_t::HEGEL_STATUS_VALID, None),
            Ended::Invalid => (hegel_status_t::HEGEL_STATUS_INVALID, None),
            Ended::Interesting => {
                (hegel_status_t::HEGEL_STATUS_INTERESTING, Some(text(origin)))
            }
        };
        self.done = true;
        // SAFETY: self.raw is live; origin outlives the call.
        check(with_context(|ctx| unsafe {
            hegel_c::hegel_mark_complete(
                ctx,
                self.raw,
                status as u32,
                origin
                    .as_ref()
                    .map_or(ptr::null(), |origin| origin.as_ptr()),
            )
        }))
    }

    /// A draw's code: `Ok`, or the error the Luau reads. Running out of
    /// choices is remembered, so the case ends as an overrun.
    fn drew(&mut self, rc: hegel_result_t) -> Result<(), String> {
        match rc {
            hegel_result_t::HEGEL_OK => Ok(()),
            hegel_result_t::HEGEL_E_STOP_TEST => {
                self.overran = true;
                Err("hegel: this case ran out of choices".to_owned())
            }
            _ => Err(last_error()),
        }
    }

    /// What the plugin's Luau asked of its case, through the `hegel`
    /// tool: `op` names it.
    pub fn call(&mut self, args: &Value) -> Result<Value, String> {
        if self.done {
            return Err("hegel: this case is over".to_owned());
        }
        self.calls += 1;
        if self.calls > MOST_CALLS {
            self.overran = true;
            return Err("hegel: this case ran out of choices".to_owned());
        }
        let number = |name: &str| args[name].as_f64();
        let count = |name: &str| args[name].as_u64();
        let id = |list: usize| -> Result<usize, String> {
            args["id"]
                .as_u64()
                .map(|id| id as usize)
                .filter(|id| (1..=list).contains(id))
                .map(|id| id - 1)
                .ok_or_else(|| "hegel: no such handle".to_owned())
        };
        let raw = self.raw;
        match args["op"].as_str().unwrap_or_default() {
            "integer" => {
                let min = number("min").map_or(-EXACT, |n| n as i64);
                let max = number("max").map_or(EXACT, |n| n as i64);
                if min > max {
                    return Err(format!(
                        "integers: min {min} is above max {max}"
                    ));
                }
                let mut out = 0;
                // SAFETY: raw is live; out is valid.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_generate_integer(
                        ctx, raw, min, max, &mut out,
                    )
                });
                self.drew(rc)?;
                Ok(json!(out))
            }
            "boolean" => {
                let p = number("p").unwrap_or(0.5);
                let mut out = false;
                // SAFETY: raw is live; out is valid.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_generate_boolean(
                        ctx, raw, p, false, false, &mut out,
                    )
                });
                self.drew(rc)?;
                Ok(json!(out))
            }
            "float" => {
                let min = number("min").unwrap_or(-f64::MAX);
                let max = number("max").unwrap_or(f64::MAX);
                let mut out = 0.0;
                // SAFETY: raw is live; out is valid. JSON holds no NaN nor
                // infinity, so neither is drawn.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_generate_float(
                        ctx, raw, 64, min, max, false, false, false, false,
                        5e-324, &mut out,
                    )
                });
                self.drew(rc)?;
                Ok(json!(out))
            }
            "text" => {
                let min = count("min").unwrap_or(0);
                let max = count("max").unwrap_or(u64::MAX);
                let codec = args["codec"].as_str().map(text);
                let mut generator = ptr::null_mut();
                // SAFETY: every pointer is valid or null, as the engine
                // allows; codec outlives the call.
                check(with_context(|ctx| unsafe {
                    hegel_c::hegel_string_generator_text(
                        ctx,
                        min,
                        max,
                        codec
                            .as_ref()
                            .map_or(ptr::null(), |codec| codec.as_ptr()),
                        0,
                        u32::MAX,
                        ptr::null(),
                        0,
                        ptr::null(),
                        0,
                        ptr::null(),
                        0,
                        ptr::null(),
                        0,
                        &mut generator,
                    )
                }))?;
                self.string(generator)
            }
            "regex" => {
                let pattern =
                    text(args["pattern"].as_str().unwrap_or_default());
                let mut generator = ptr::null_mut();
                // SAFETY: pattern outlives the call; no alphabet.
                check(with_context(|ctx| unsafe {
                    hegel_c::hegel_string_generator_regex(
                        ctx,
                        pattern.as_ptr(),
                        true,
                        ptr::null(),
                        &mut generator,
                    )
                }))?;
                self.string(generator)
            }
            "span_start" => {
                let label = text(args["label"].as_str().unwrap_or("value"));
                let mut id = 0;
                // SAFETY: label outlives the call; raw is live.
                check(with_context(|ctx| unsafe {
                    hegel_c::hegel_label_from_name(ctx, label.as_ptr(), &mut id)
                }))?;
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_start_span(ctx, raw, id)
                });
                self.drew(rc)?;
                Ok(Value::Null)
            }
            "span_stop" => {
                let discard = args["discard"].as_bool().unwrap_or(false);
                // SAFETY: raw is live.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_stop_span(ctx, raw, discard)
                });
                self.drew(rc)?;
                Ok(Value::Null)
            }
            "collection" => {
                let min = count("min").unwrap_or(0);
                let max = count("max").unwrap_or(u64::MAX);
                let mut collection = ptr::null_mut();
                // SAFETY: raw is live; the collection is freed with the case.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_new_collection(
                        ctx,
                        raw,
                        min,
                        max,
                        &mut collection,
                    )
                });
                self.drew(rc)?;
                self.collections.push(collection);
                Ok(json!(self.collections.len()))
            }
            "more" => {
                let collection = self.collections[id(self.collections.len())?];
                let mut more = false;
                // SAFETY: both handles are live.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_collection_more(
                        ctx, raw, collection, &mut more,
                    )
                });
                self.drew(rc)?;
                Ok(json!(more))
            }
            "reject" => {
                let collection = self.collections[id(self.collections.len())?];
                let why = text("a duplicate");
                // SAFETY: both handles are live; why outlives the call.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_collection_reject(
                        ctx,
                        raw,
                        collection,
                        why.as_ptr(),
                    )
                });
                self.drew(rc)?;
                Ok(Value::Null)
            }
            "machine" => {
                let names: Vec<CString> = args["rules"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|name| text(name.as_str().unwrap_or("rule")))
                    .collect();
                if names.is_empty() {
                    return Err("a machine needs a rule".to_owned());
                }
                let pointers: Vec<*const c_char> =
                    names.iter().map(|name| name.as_ptr()).collect();
                let groups = vec![0_i64; names.len()];
                let weights: Vec<f64> = (0..names.len())
                    .map(|at| args["weights"][at].as_f64().unwrap_or(1.0))
                    .collect();
                let steps = args["steps"].as_i64().unwrap_or(50).max(1);
                let (mut machine, mut workers) = (ptr::null_mut(), 0);
                // SAFETY: the arrays hold names.len() entries and outlive
                // the call; no invariants; one worker, so a sequential
                // machine.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_new_state_machine(
                        ctx,
                        raw,
                        pointers.as_ptr(),
                        groups.as_ptr(),
                        weights.as_ptr(),
                        names.len(),
                        ptr::null(),
                        ptr::null(),
                        0,
                        1,
                        1,
                        steps,
                        &mut machine,
                        &mut workers,
                    )
                });
                self.drew(rc)?;
                self.machines.push(machine);
                Ok(json!(self.machines.len()))
            }
            "round" => {
                let machine = self.machines[id(self.machines.len())?];
                let mut group = 0;
                // SAFETY: both handles are live.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_state_machine_next_group(
                        ctx, raw, machine, &mut group,
                    )
                });
                self.drew(rc)?;
                Ok(json!(group != hegel_c::HEGEL_STATE_MACHINE_DONE))
            }
            "rule" => {
                let machine = self.machines[id(self.machines.len())?];
                let mut rule = 0;
                // SAFETY: both handles are live; worker 0 of one.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_state_machine_next_rule(
                        ctx, raw, machine, 0, &mut rule,
                    )
                });
                self.drew(rc)?;
                Ok(if rule == hegel_c::HEGEL_STATE_MACHINE_DONE {
                    Value::Null
                } else {
                    json!(rule + 1)
                })
            }
            "rejected" => {
                let machine = self.machines[id(self.machines.len())?];
                // SAFETY: both handles are live; worker 0 of one.
                let rc = with_context(|ctx| unsafe {
                    hegel_c::hegel_state_machine_rule_rejected(
                        ctx, raw, machine, 0,
                    )
                });
                self.drew(rc)?;
                Ok(Value::Null)
            }
            other => Err(format!("hegel has no `{other}`")),
        }
    }

    /// A string drawn from `generator`, which is then freed.
    fn string(
        &mut self,
        generator: *mut HegelStringGenerator,
    ) -> Result<Value, String> {
        let mut result = hegel_c::hegel_generate_string_result_t {
            data: ptr::null_mut(),
            len: 0,
        };
        let raw = self.raw;
        // SAFETY: generator and raw are live; the result's buffer is
        // copied out, then it and the generator are freed once.
        let rc = with_context(|ctx| unsafe {
            hegel_c::hegel_generate_string(ctx, raw, generator, &mut result)
        });
        let drawn = self.drew(rc).map(|()| {
            let bytes = unsafe {
                std::slice::from_raw_parts(result.data.cast::<u8>(), result.len)
            };
            json!(String::from_utf8_lossy(bytes))
        });
        with_context(|ctx| unsafe {
            if drawn.is_ok() {
                let _ = hegel_c::hegel_generate_string_result_free(
                    ctx,
                    &mut result,
                );
            }
            let _ = hegel_c::hegel_string_generator_free(ctx, generator);
        });
        drawn
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        with_context(|ctx| unsafe {
            // SAFETY: each handle came from the engine for this case and
            // is freed once; a case not ended is ended as an overrun, as
            // the engine needs every case it offered ended.
            if !self.done {
                let _ = hegel_c::hegel_mark_complete(
                    ctx,
                    self.raw,
                    hegel_status_t::HEGEL_STATUS_OVERRUN as u32,
                    ptr::null(),
                );
            }
            for collection in self.collections.drain(..) {
                let _ = hegel_c::hegel_collection_free(ctx, collection);
            }
            for machine in self.machines.drain(..) {
                let _ = hegel_c::hegel_state_machine_free(ctx, machine);
            }
            let _ = hegel_c::hegel_test_case_free(ctx, self.raw);
        });
    }
}

/// What tells one failure from another: the line it was raised at, when
/// the error says; else its first line with the numbers taken out, so
/// what each case drew does not make it a new failure.
fn origin(error: &str) -> String {
    let first = error.lines().next().unwrap_or_default();
    if let Some(rest) = first.strip_prefix("codemode:") {
        let line: String =
            rest.chars().take_while(char::is_ascii_digit).collect();
        if !line.is_empty() {
            return format!("line {line}");
        }
    }
    first
        .chars()
        .map(|c| if c.is_ascii_digit() { '#' } else { c })
        .take(200)
        .collect()
}

/// A failed case's report: its error, then what it drew and noted.
fn report(error: &str, notes: &Value) -> String {
    let mut out = error.to_owned();
    for note in notes.as_array().into_iter().flatten() {
        out.push_str("\n  ");
        out.push_str(note.as_str().unwrap_or_default());
    }
    out
}

/// A property's run as its VM drives it, every case in the one VM: the
/// engine's run, the case going on, and the report to keep.
pub struct Driver {
    run: Option<Run>,
    /// The case going on, and whether the engine wants its report kept.
    case: Option<(Case, bool)>,
    /// The failing case's report: the one the engine marks to keep, the
    /// last replay of the shrunk failure, or else the latest found.
    kept: Option<String>,
    deadline: Instant,
    limit: Duration,
    /// Whether the run stopped at its deadline.
    ran_out: bool,
    /// Whether the engine said the run is over.
    drained: bool,
    /// The engine's error, which ended the run.
    broken: Option<String>,
}

/// The driver the host gives a property's VM. Its lock is never held
/// across an await: [`next`] takes the run out while the engine works.
pub type Shared = Arc<Mutex<Driver>>;

/// What the property's Luau asked through the `hegel` tool: `next` for
/// the next case (false once there is none), `end` to say how it ended,
/// or a draw from it.
pub async fn call(driver: &Shared, args: &Value) -> Result<Value, String> {
    if args["op"].as_str() == Some("next") {
        return next(driver).await;
    }
    let mut driver = driver.lock().expect("not poisoned");
    match args["op"].as_str().unwrap_or_default() {
        "end" => driver.end(args),
        _ => match &mut driver.case {
            Some((case, _)) => case.call(args),
            None => Err("hegel: no case is going on".to_owned()),
        },
    }
}

/// The next case: the run is taken out of the driver while the engine
/// works, off the runtime's threads, and put back.
async fn next(driver: &Shared) -> Result<Value, String> {
    let mut run = {
        let mut driver = driver.lock().expect("not poisoned");
        if driver.case.is_some() {
            return Err("hegel: the case going on has not ended".to_owned());
        }
        if driver.drained || driver.broken.is_some() {
            return Ok(json!(false));
        }
        if Instant::now() > driver.deadline {
            driver.ran_out = true;
            return Ok(json!(false));
        }
        driver.run.take().ok_or("hegel: the run is over")?
    };
    let (run, next) = tokio::task::spawn_blocking(move || {
        let next = run.next_case();
        (run, next)
    })
    .await
    .map_err(|error| format!("hegel: {error}"))?;
    let mut driver = driver.lock().expect("not poisoned");
    driver.run = Some(run);
    match next {
        Ok(Some(case)) => {
            let capture = case.should_capture();
            driver.case = Some((case, capture));
            Ok(json!(true))
        }
        Ok(None) => {
            driver.drained = true;
            Ok(json!(false))
        }
        Err(error) => {
            driver.broken = Some(error);
            Ok(json!(false))
        }
    }
}

impl Driver {
    /// Drives `run`, stopping after `limit`.
    pub fn new(run: Run, limit: Duration) -> Self {
        Self {
            run: Some(run),
            case: None,
            kept: None,
            deadline: Instant::now() + limit,
            limit,
            ran_out: false,
            drained: false,
            broken: None,
        }
    }

    fn end(&mut self, args: &Value) -> Result<Value, String> {
        let (mut case, capture) =
            self.case.take().ok_or("hegel: no case is going on")?;
        let ended = match args["status"].as_str() {
            Some("valid") => Ended::Valid,
            Some("invalid") => Ended::Invalid,
            _ => Ended::Interesting,
        };
        let error = args["error"].as_str().unwrap_or("it failed");
        let overran = case.overran();
        if let Err(error) = case.complete(ended, &origin(error)) {
            self.broken = Some(error);
        }
        if ended == Ended::Interesting
            && !overran
            && (capture || self.kept.is_none())
        {
            self.kept = Some(report(error, &args["notes"]));
        }
        Ok(Value::Null)
    }

    /// Why the property failed, once its VM is done: `failed` says why
    /// the VM itself did, if it did. `None` when it held.
    pub fn failure(&mut self, failed: Option<String>) -> Option<String> {
        if let Some(why) = failed {
            self.case = None;
            return Some(match self.kept.take() {
                Some(kept) => {
                    format!("{why}\n(before that, it failed so)\n{kept}")
                }
                None => why,
            });
        }
        if let Some(error) = self.broken.take() {
            return Some(format!("hegel: {error}"));
        }
        if self.ran_out || !self.drained {
            let ran_out = format!(
                "(it stopped after {} s, before hegel was done with it)",
                self.limit.as_secs()
            );
            return self.kept.take().map(|kept| format!("{kept}\n{ran_out}"));
        }
        let run = self.run.as_ref()?;
        match run.verdict() {
            Ok(Verdict::Passed) => None,
            Ok(Verdict::Failed(failures)) => {
                let mut out =
                    self.kept.take().unwrap_or_else(|| "it failed".to_owned());
                for failure in failures {
                    if let Some(caveat) = failure.caveat {
                        out.push_str(&format!("\n{caveat}"));
                    }
                    if let Some(blob) = failure.blob {
                        out.push_str(&format!(
                            "\nreproduce: {{ reproduce = \"{blob}\" }}"
                        ));
                    }
                }
                Some(out)
            }
            Ok(Verdict::Broken(error)) => {
                let lines = run.lines().join("\n");
                Some(if lines.is_empty() {
                    error
                } else {
                    format!("{error}\n{lines}")
                })
            }
            Err(error) => Some(format!("hegel: {error}")),
        }
    }
}
