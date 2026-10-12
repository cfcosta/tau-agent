//! The arms: what the model can do about an input too large to read.

use std::{path::Path, sync::Arc, time::Duration};

use futures_util::future::BoxFuture;
use serde::Serialize;
use tau_agent::{agent::Agent, limits::Limits};
use tau_ai::{
    llm::{Llm, LlmError, LlmSession},
    model::find,
    responses::request::Settings,
};
use tau_codemode_host::{Codemode, inference_budget};
use tau_compaction::Compaction;
use tau_tools_host::{path::Root, plugin::CodingTools};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Arm {
    /// The coding tools alone.
    Plain,
    /// Plus `codemode` with `tools.infer`, at its default limits.
    Codemode,
    /// Codemode with the limits raised and nothing else changed: what the
    /// default limits cost.
    CodemodeWide,
    /// Codemode with the limits raised and the strategy of the paper in
    /// the instructions.
    Rlm,
}

impl Arm {
    pub const ALL: [Arm; 4] =
        [Arm::Plain, Arm::Codemode, Arm::CodemodeWide, Arm::Rlm];

    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Codemode => "codemode",
            Self::CodemodeWide => "codemode_wide",
            Self::Rlm => "rlm",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|arm| arm.name() == name)
    }

    pub fn rank(self) -> usize {
        Self::ALL
            .iter()
            .position(|arm| *arm == self)
            .expect("every arm is listed")
    }

    fn codemode(self) -> bool {
        self != Self::Plain
    }

    fn wide(self) -> bool {
        matches!(self, Self::CodemodeWide | Self::Rlm)
    }

    pub fn instructions(self) -> String {
        match self {
            Self::Rlm => format!("{INSTRUCTIONS} {STRATEGY}"),
            _ => INSTRUCTIONS.to_owned(),
        }
    }
}

/// tau-ui's instructions, without the version control tools this
/// evaluation leaves out.
pub const INSTRUCTIONS: &str = "You are tau, a coding agent working in the \
    user's repository. Use the tools to read and change files and to run \
    commands. Be concise, and say which tests you ran.";

/// What the `rlm` arm adds: the way the paper's models work, as a worked
/// example for the tools tau has.
pub const STRATEGY: &str = r#"
When the input is too large to read through, do not read it into your own context. Work on it from a `codemode` script instead, and keep only small results:
1. Look at its shape first (`tools.exec_command` with `wc -l` and `head`).
2. Cut it into chunks of a few hundred lines with `tools.read({ path = ..., offset = first_line, limit = n })`.
3. Ask a sub-model about each chunk with `tools.infer`, many at once through `map(chunks, fn, 8)`. Give each call the chunk as `context`, a precise `task`, and a `schema` so the answer is data, for example one label per line, or a count with the line numbers behind it.
4. Combine the answers in code, check them (the labels cover every line once; the counts add up), and `return` only the result.
Retry the chunks whose call failed (`ok = false`) before you answer. Never answer from a sample.
Example:
```luau
local total, size = 4000, 400
local starts = array({})
for first = 1, total, size do table.insert(starts, first) end
local results = map(starts, function(first)
    local chunk = tools.read({ path = "data/file.txt", offset = first, limit = size })
    return tools.infer({
        task = "Count the lines that are about X. Return the count.",
        context = chunk,
        schema = { type = "object", properties = { count = { type = "integer" } }, required = { "count" } },
    })
end, 8)
local sum = 0
for _, r in ipairs(results) do
    assert(r.ok and r.value.ok, "a chunk failed")
    sum += r.value.value.count
end
return sum
```"#;

/// The limits of the `codemode_wide` and `rlm` arms on `tools.infer`:
/// enough for a few hundred chunks.
pub fn wide_limits() -> inference_budget::Limits {
    inference_budget::Limits {
        max_calls: 400,
        max_concurrency: 8,
        timeout: Duration::from_secs(20 * 60),
        max_tokens: None,
        max_cost_usd: Some(10.0),
    }
}

/// A model shared by runs: [`Agent::new`] takes a sized [`Llm`].
#[derive(Clone)]
pub struct SharedLlm(pub Arc<dyn Llm>);

impl Llm for SharedLlm {
    fn open(
        &self,
        settings: Settings,
    ) -> BoxFuture<'static, Result<Box<dyn LlmSession>, LlmError>> {
        self.0.open(settings)
    }
}

/// The agent for one run in an arm, working in `repo`.
pub fn agent(
    arm: Arm,
    llm: Arc<dyn Llm>,
    model: &str,
    repo: &Path,
    limits: Limits,
) -> Agent {
    let mut compaction = Compaction::default();
    if let Some(model) = find(model) {
        compaction = compaction.context_window(model.context_window);
    }
    let mut agent = Agent::new(SharedLlm(llm))
        .name("coder")
        .model(model)
        .instructions(arm.instructions())
        .limits(limits)
        .plugin(compaction)
        .plugin(CodingTools::new(Root::new(repo.to_owned())));
    if arm.codemode() {
        let mut codemode = Codemode::new(None);
        if arm.wide() {
            codemode = codemode.with_inference_limits(wide_limits());
        }
        agent = agent.plugin(codemode);
    }
    agent
}
