//! The agent half: the skills listed in a run's instructions as it
//! starts, and the `skill` tool that loads one.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use serde_json::{Value, json};
use tau_agent::{
    error::ToolError,
    plugin::{Plugin, PluginCtx, PluginError, PluginRun, RunPlan},
    tool::{AgentTool, ToolCtx, ToolOutput},
};

use crate::{Loaded, NAME, SKILL_FILE, Skills, TOOL, scan};

/// What the model reads of the tool.
const DESCRIPTION: &str = "Loads a skill: its instructions, from its \
    SKILL.md, and the folder its other files are in. Load one when a task \
    fits its description in the instructions' list, before starting.";

static PARAMETERS: LazyLock<Value> = LazyLock::new(|| {
    json!({
        "type": "object",
        "properties": {
            "name": {
                "type": "string",
                "description": "The skill's name, as the list gives it."
            }
        },
        "required": ["name"],
        "additionalProperties": false
    })
});

/// The plugin for one run, over the skills the folder held as it
/// started: the list in its instructions, and the tool.
pub struct SkillsPlugin {
    skills: Arc<Skills>,
}

impl SkillsPlugin {
    pub fn new(skills: Skills) -> Self {
        Self {
            skills: Arc::new(skills),
        }
    }
}

#[async_trait]
impl Plugin for SkillsPlugin {
    fn name(&self) -> &str {
        NAME
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        vec![Arc::new(SkillTool {
            skills: self.skills.clone(),
        })]
    }

    /// Lists the skills in the run's instructions. They are fixed for
    /// the run's session, so a skill added meanwhile is listed the next
    /// time a run starts.
    async fn start(
        &self,
        plan: &mut RunPlan,
        _ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        if let Some(section) = scan::section(&self.skills) {
            plan.instructions = Some(match plan.instructions.take() {
                Some(base) if !base.is_empty() => {
                    format!("{base}\n\n{section}")
                }
                _ => section,
            });
        }
        Ok(Box::new(()))
    }
}

/// Loads a listed skill's instructions, as its `SKILL.md` has them now.
struct SkillTool {
    skills: Arc<Skills>,
}

#[async_trait]
impl AgentTool for SkillTool {
    fn name(&self) -> &str {
        TOOL
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> &Value {
        &PARAMETERS
    }

    async fn call(
        &self,
        args: Value,
        _ctx: ToolCtx,
    ) -> Result<ToolOutput, ToolError> {
        let name = args["name"].as_str().unwrap_or_default().to_owned();
        let Some(skill) = self.skills.get(&name).cloned() else {
            let names: Vec<&str> = self
                .skills
                .found
                .iter()
                .map(|skill| skill.name.as_str())
                .collect();
            return Err(ToolError::Message(format!(
                "There is no skill {name:?}. The skills are: {}.",
                names.join(", ")
            )));
        };
        let file = skill.dir.join(SKILL_FILE);
        let read = file.clone();
        let text =
            tokio::task::spawn_blocking(move || std::fs::read_to_string(read))
                .await
                .map_err(|error| ToolError::Message(error.to_string()))?
                .map_err(|error| {
                    ToolError::Message(format!(
                        "Cannot read {}: {error}",
                        file.display()
                    ))
                })?;
        let body = scan::split(&text).map(|(_, body)| body).unwrap_or(&text);
        let mut output = ToolOutput::text(format!(
            "Skill {name}. Its folder, where the files it names are: {}\n\n{}",
            skill.dir.display(),
            body.trim()
        ));
        output.details = Some(
            serde_json::to_value(Loaded {
                name,
                description: skill.description,
                file,
            })
            .expect("plain JSON"),
        );
        Ok(output)
    }
}
