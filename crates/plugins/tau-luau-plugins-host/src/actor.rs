//! A plugin's actor (ADR 0056): one VM kept as long as its model, taking
//! one message a step.
//!
//! - **The VM** has every safe library but the clock and randomness, the
//!   pure globals (`json`, `array`), and a `require` over the plugin's own
//!   modules. It reaches nothing: what a hook does outside it is a
//!   command it gives, which the host carries out.
//! - **A step** is `tau._actor`'s `deliver`, in Luau, so the host and the
//!   test harness run the same code. It runs to its end, an await, or its
//!   deadline, which fails it.
//! - **Drawing** (`view`, a card, a page) reads a frozen copy of the
//!   model and waits on nothing.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use mlua::{
    Function,
    Lua,
    LuaOptions,
    StdLib,
    Table,
    Value as LuaValue,
    VmState,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tau_codemode_host::value::{from_lua, to_lua};

use crate::runtime::Modules;

/// The memory a plugin's VM may hold.
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;

/// What a hook gives the host to carry out: the host call it makes, and
/// its args. `all` carries several, answered together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub op: String,
    #[serde(default)]
    pub args: Value,
}

/// How a hook ended: what it returned, or why it failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Ended {
    Value { value: Value },
    Error { error: String },
}

/// What one step did.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Step {
    /// The model after it.
    pub model: Value,
    /// Whether it changed the model.
    pub changed: bool,
    /// The hooks that ended in it, by id.
    pub ended: BTreeMap<String, Ended>,
    /// The command each hook it left waiting awaits, by id.
    pub waits: BTreeMap<String, Command>,
    /// The commands it gave that need no answer, in order.
    pub given: Vec<Command>,
    pub logs: Vec<String>,
}

/// A message for a model's mailbox.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Message {
    /// Starts the hook at `path` as `id`, with `input` and `env`.
    Call {
        id: String,
        path: Vec<String>,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        env: Value,
    },
    /// The answer to what `id` awaits.
    Answer {
        id: String,
        ok: bool,
        #[serde(default)]
        values: Vec<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Ends `id`, its await raising `why`.
    Cancel { id: String, why: String },
}

impl Message {
    /// The hook it is for.
    pub fn id(&self) -> &str {
        match self {
            Self::Call { id, .. }
            | Self::Answer { id, .. }
            | Self::Cancel { id, .. } => id,
        }
    }
}

/// A plugin's VM, its modules loaded and its actor made.
pub struct Actor {
    lua: Lua,
    actor: Table,
    deadline: Arc<Mutex<Option<(Instant, Duration)>>>,
}

impl std::fmt::Debug for Actor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Actor").finish_non_exhaustive()
    }
}

impl Actor {
    /// The actor of the plugin whose entry module is `entry`, among
    /// `modules`, on `model`.
    pub fn new(
        modules: &Modules,
        entry: &str,
        model: &Value,
    ) -> Result<Self, String> {
        let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
            .map_err(|error| error.to_string())?;
        let deadline: Arc<Mutex<Option<(Instant, Duration)>>> = Arc::default();
        let made = (|| -> mlua::Result<Table> {
            tau_codemode_host::install_pure_globals(&lua)?;
            let globals = lua.globals();
            globals.raw_remove("require")?;
            // Steps are deterministic: no clock, no randomness.
            let os: Table = globals.get("os")?;
            for name in ["time", "clock", "date"] {
                os.raw_remove(name)?;
            }
            let math: Table = globals.get("math")?;
            for name in ["random", "randomseed"] {
                math.raw_remove(name)?;
            }
            install_require(&lua, modules)?;
            lua.sandbox(true)?;
            lua.set_memory_limit(MEMORY_LIMIT)?;
            let watched = Arc::clone(&deadline);
            // Once per step: what the actor does after the hook fails, its
            // rollback, runs on.
            lua.set_interrupt(move |_| {
                let mut deadline = watched.lock().expect("not poisoned");
                if let Some((at, limit)) = *deadline
                    && Instant::now() >= at
                {
                    *deadline = None;
                    return Err(mlua::Error::runtime(format!(
                        "it ran past its {} ms",
                        limit.as_millis()
                    )));
                }
                Ok(VmState::Continue)
            });
            let require: Function = lua.globals().get("require")?;
            let tau: Table = require.call("tau")?;
            let plugin: LuaValue = require.call(entry)?;
            let make: Function = tau.get("_actor")?;
            make.call((plugin, to_lua(&lua, model)?))
        })();
        let actor =
            made.map_err(|error| tau_codemode_host::error_text(&error))?;
        Ok(Self {
            lua,
            actor,
            deadline,
        })
    }

    /// Takes `message` in one step, which may run for `limit`.
    pub fn deliver(
        &self,
        message: &Message,
        limit: Duration,
    ) -> Result<Step, String> {
        let message =
            serde_json::to_value(message).map_err(|error| error.to_string())?;
        let value = self.with_limit(limit, || {
            let deliver: Function = self.actor.get("deliver")?;
            deliver.call::<LuaValue>(to_lua(&self.lua, &message)?)
        })?;
        step_of(&self.lua, &value)
    }

    /// What the drawing at `path` draws, for `limit`, from a frozen copy
    /// of the model.
    pub fn draw(
        &self,
        path: &[String],
        input: Option<&Value>,
        env: &Value,
        limit: Duration,
    ) -> Result<Value, String> {
        let value = self.with_limit(limit, || {
            let draw: Function = self.actor.get("draw")?;
            let input = match input {
                Some(input) => to_lua(&self.lua, input)?,
                None => LuaValue::Nil,
            };
            draw.call::<LuaValue>((
                to_lua(&self.lua, &json!(path))?,
                input,
                to_lua(&self.lua, env)?,
            ))
        })?;
        from_lua(&self.lua, &value).map_err(|error| error.to_string())
    }

    /// The model as it is.
    pub fn model(&self) -> Result<Value, String> {
        let model: LuaValue =
            self.actor.get("model").map_err(|error| error.to_string())?;
        from_lua(&self.lua, &model).map_err(|error| error.to_string())
    }

    /// The command each hook under way awaits, by id.
    pub fn waiting(&self) -> Result<BTreeMap<String, Command>, String> {
        let value = self.with_limit(Duration::from_secs(1), || {
            let waiting: Function = self.actor.get("waiting")?;
            waiting.call::<LuaValue>(())
        })?;
        let value =
            from_lua(&self.lua, &value).map_err(|error| error.to_string())?;
        commands_by_id(&value)
    }

    fn with_limit<T>(
        &self,
        limit: Duration,
        run: impl FnOnce() -> mlua::Result<T>,
    ) -> Result<T, String> {
        *self.deadline.lock().expect("not poisoned") =
            Some((Instant::now() + limit, limit));
        let result = run();
        *self.deadline.lock().expect("not poisoned") = None;
        result.map_err(|error| tau_codemode_host::error_text(&error))
    }
}

/// `require(name)` over `modules`: each loaded once, its value kept.
fn install_require(lua: &Lua, modules: &Modules) -> mlua::Result<()> {
    let sources: BTreeMap<String, (String, String)> = modules
        .iter()
        .map(|(name, definition)| {
            (
                name.clone(),
                (
                    definition.source().to_owned(),
                    definition.version().to_owned(),
                ),
            )
        })
        .collect();
    let loaded = lua.create_table()?;
    let require = lua.create_function(move |lua, name: String| {
        if let Some(value) =
            loaded.raw_get::<Option<LuaValue>>(name.as_str())?
        {
            return Ok(value);
        }
        let Some((source, version)) = sources.get(&name) else {
            return Err(mlua::Error::runtime(format!("no module {name}")));
        };
        let value: LuaValue = lua
            .load(source.as_str())
            .set_name(format!("=module:{name}@{version}"))
            .set_mode(mlua::chunk::ChunkMode::Text)
            .eval()?;
        loaded.raw_set(name.as_str(), value.clone())?;
        Ok(value)
    })?;
    lua.globals().set("require", require)
}

fn step_of(lua: &Lua, value: &LuaValue) -> Result<Step, String> {
    let LuaValue::Table(table) = value else {
        return Err("the actor gave no step".into());
    };
    let field = |name: &str| -> Result<Value, String> {
        let lua_value: LuaValue =
            table.get(name).map_err(|error| error.to_string())?;
        from_lua(lua, &lua_value)
    };
    let ended = match field("ended")? {
        Value::Object(map) => map
            .into_iter()
            .map(|(id, ended)| {
                serde_json::from_value(ended)
                    .map(|ended| (id, ended))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<_, _>>()?,
        _ => BTreeMap::new(),
    };
    let commands = |value: Value| -> Result<Vec<Command>, String> {
        match value {
            Value::Array(items) => items
                .into_iter()
                .map(|item| {
                    serde_json::from_value(item)
                        .map_err(|error| error.to_string())
                })
                .collect(),
            _ => Ok(Vec::new()),
        }
    };
    Ok(Step {
        model: field("model")?,
        changed: field("changed")? == Value::Bool(true),
        ended,
        waits: commands_by_id(&field("waits")?)?,
        given: commands(field("given")?)?,
        logs: match field("logs")? {
            Value::Array(lines) => lines
                .into_iter()
                .filter_map(|line| line.as_str().map(str::to_owned))
                .collect(),
            _ => Vec::new(),
        },
    })
}

fn commands_by_id(value: &Value) -> Result<BTreeMap<String, Command>, String> {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(id, command)| {
                serde_json::from_value(command.clone())
                    .map(|command| (id.clone(), command))
                    .map_err(|error| error.to_string())
            })
            .collect(),
        _ => Ok(BTreeMap::new()),
    }
}
