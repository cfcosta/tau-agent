//! The run's tools as the loop holds them, and the order a batch's calls start in.

use super::*;

/// A tool as the loop holds it: the tool, its compiled schema, and the
/// index of the plugin that added it.
#[derive(Clone)]
pub(crate) struct LoopTool {
    pub tool: Arc<dyn AgentTool>,
    pub schema: Arc<ArgumentSchema>,
    pub owner: Option<usize>,
}

/// The text of a call to a tool the caller cannot reach.
pub(crate) fn not_found(name: &str) -> String {
    format!("Tool {name} not found")
}

/// The text of a nested call to a `ModelOnly` tool.
pub(crate) fn model_only(name: &str) -> String {
    format!("Tool {name} cannot be called from a tool")
}

/// A run's tools, fixed once it starts, and the plugins' tool sources,
/// asked by name when a tool calls one.
pub(crate) struct Toolbox {
    /// One per name, in the order they were added.
    tools: Vec<LoopTool>,
    by_name: HashMap<String, usize>,
    /// Each source, with the index of its plugin.
    sources: Vec<(Arc<dyn ToolSource>, usize)>,
}

impl Toolbox {
    /// A toolbox of `tools`; of two tools of one name, the later wins,
    /// in the earlier's place.
    pub(crate) fn new(
        tools: Vec<LoopTool>,
        sources: Vec<(Arc<dyn ToolSource>, usize)>,
    ) -> Self {
        let mut unique: Vec<LoopTool> = Vec::with_capacity(tools.len());
        let mut by_name = HashMap::new();
        for tool in tools {
            match by_name.get(tool.tool.name()) {
                Some(&at) => unique[at] = tool,
                None => {
                    by_name.insert(tool.tool.name().to_owned(), unique.len());
                    unique.push(tool);
                }
            }
        }
        Self {
            tools: unique,
            by_name,
            sources,
        }
    }

    /// The tools declared to the model, in order.
    pub(crate) fn declared(&self) -> impl Iterator<Item = &Arc<dyn AgentTool>> {
        self.tools
            .iter()
            .map(|tool| &tool.tool)
            .filter(|tool| tool.exposure().declared())
    }

    /// The tool the model calls by `name`: only a declared one.
    pub(crate) fn for_model(&self, name: &str) -> Result<LoopTool, String> {
        self.by_name
            .get(name)
            .map(|&at| &self.tools[at])
            .filter(|tool| tool.tool.exposure().declared())
            .cloned()
            .ok_or_else(|| not_found(name))
    }

    /// The tool a tool calls by `name`: the run's tool of that name if
    /// it is callable, else the first callable one the sources offer, as
    /// [`Self::catalog`] lists it.
    pub(crate) fn for_tool(&self, name: &str) -> Result<LoopTool, String> {
        if let Some(&at) = self.by_name.get(name) {
            let tool = &self.tools[at];
            return if tool.tool.exposure().callable() {
                Ok(tool.clone())
            } else {
                Err(model_only(name))
            };
        }
        let mut model_only_seen = false;
        for (source, owner) in &self.sources {
            for tool in source.tools() {
                if tool.name() != name {
                    continue;
                }
                if !tool.exposure().callable() {
                    model_only_seen = true;
                    continue;
                }
                let schema = ArgumentSchema::new(tool.parameters()).map_err(
                    |error| {
                        format!("Tool {name} has an invalid schema: {error}")
                    },
                )?;
                return Ok(LoopTool {
                    tool,
                    schema: Arc::new(schema),
                    owner: Some(*owner),
                });
            }
        }
        Err(if model_only_seen {
            model_only(name)
        } else {
            not_found(name)
        })
    }

    /// What a tool can call now.
    pub(crate) fn catalog(&self) -> Catalog {
        let mut tools: Vec<Arc<dyn AgentTool>> = self
            .tools
            .iter()
            .map(|tool| tool.tool.clone())
            .filter(|tool| tool.exposure().callable())
            .collect();
        let mut namespaces = Vec::new();
        for (source, _) in &self.sources {
            for tool in source.tools() {
                let hidden = self.by_name.contains_key(tool.name())
                    || tools.iter().any(|t| t.name() == tool.name());
                if tool.exposure().callable() && !hidden {
                    tools.push(tool);
                }
            }
            namespaces.extend(source.namespaces());
        }
        let sources = self
            .sources
            .iter()
            .map(|(source, _)| source.clone())
            .collect();
        Catalog::new(tools, namespaces, sources)
    }
}

/// A call ready to run: its index in the batch, its tool, and the call.
pub(super) type Ready = (usize, LoopTool, ToolCall);

/// A batch's ready calls, in the order they start, and the sizes of the
/// groups they start in: each group starts once the one before is done.
/// A sequential tool makes every call a group of its own; otherwise each
/// grouped tool's calls form a group, and the calls to all other tools
/// another, ordered by their first calls.
pub(super) fn schedule(
    ready: Vec<Ready>,
) -> (VecDeque<Ready>, VecDeque<usize>) {
    let modes: Vec<ExecutionMode> = ready
        .iter()
        .map(|(_, tool, _)| tool.tool.execution_mode())
        .collect();
    if modes.contains(&ExecutionMode::Sequential) {
        let sizes = vec![1; ready.len()].into();
        return (ready.into(), sizes);
    }
    // Groups by key: a grouped tool's name, or `None` for the rest.
    let mut groups: Vec<(Option<String>, Vec<Ready>)> = Vec::new();
    for (call, mode) in ready.into_iter().zip(modes) {
        let key = (mode == ExecutionMode::Grouped)
            .then(|| call.1.tool.name().to_owned());
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, calls)) => calls.push(call),
            None => groups.push((key, vec![call])),
        }
    }
    let sizes = groups.iter().map(|(_, calls)| calls.len()).collect();
    let queue = groups.into_iter().flat_map(|(_, calls)| calls).collect();
    (queue, sizes)
}
