//! The kernel's gate on a plugin's agent plugins (ADR 0045): the host
//! calls only the seams the plugin declared ([`crate::Declaration`]).
//! Every agent plugin a host half gives goes through it.

use std::sync::Arc;

use async_trait::async_trait;
use tau_agent::{
    event::RunEvent,
    plugin::{
        ContextView,
        Decision,
        FinishedRun,
        Plugin,
        PluginCtx,
        PluginError,
        PluginRun,
        RequestView,
        Rewrite,
        RunPlan,
        StopDecision,
        ToolCall,
        ToolResultView,
    },
    tool::{AgentTool, ToolOutput, ToolSource},
};
use tau_ai::{
    message::{AssistantMessage, Message},
    responses::request::ReasoningEffort,
};

use crate::Seam;

/// `plugin`, called only at `seams`.
pub fn gate(plugin: Box<dyn Plugin>, seams: &[Seam]) -> Box<dyn Plugin> {
    Box::new(Gated {
        plugin,
        seams: Seams::of(seams),
    })
}

/// The seams declared, as a set.
#[derive(Clone, Copy)]
struct Seams(u16);

impl Seams {
    fn of(seams: &[Seam]) -> Self {
        Self(seams.iter().fold(0, |bits, seam| bits | Self::bit(*seam)))
    }

    fn bit(seam: Seam) -> u16 {
        let at = Seam::ALL
            .iter()
            .position(|known| *known == seam)
            .expect("every seam is in Seam::ALL");
        1 << at
    }

    fn has(self, seam: Seam) -> bool {
        self.0 & Self::bit(seam) != 0
    }
}

struct Gated {
    plugin: Box<dyn Plugin>,
    seams: Seams,
}

#[async_trait]
impl Plugin for Gated {
    fn name(&self) -> &str {
        self.plugin.name()
    }

    fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        if self.seams.has(Seam::Tools) {
            self.plugin.tools()
        } else {
            Vec::new()
        }
    }

    fn tool_source(&self) -> Option<Arc<dyn ToolSource>> {
        self.seams
            .has(Seam::Tools)
            .then(|| self.plugin.tool_source())
            .flatten()
    }

    async fn start(
        &self,
        plan: &mut RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Box<dyn PluginRun>, PluginError> {
        let run = if self.seams.has(Seam::Start) {
            self.plugin.start(plan, ctx).await?
        } else {
            // Its part in the run, without what it would change of the
            // plan.
            self.plugin.start(&mut plan.clone(), ctx).await?
        };
        Ok(Box::new(GatedRun {
            run,
            seams: self.seams,
        }))
    }

    async fn start_idle(
        &self,
        plan: &RunPlan,
        ctx: &PluginCtx,
    ) -> Result<Option<Box<dyn PluginRun>>, PluginError> {
        if !self.seams.has(Seam::StartIdle) {
            return Ok(None);
        }
        Ok(self.plugin.start_idle(plan, ctx).await?.map(|run| {
            Box::new(GatedRun {
                run,
                seams: self.seams,
            }) as Box<dyn PluginRun>
        }))
    }
}

struct GatedRun {
    run: Box<dyn PluginRun>,
    seams: Seams,
}

#[async_trait]
impl PluginRun for GatedRun {
    async fn before_tool(
        &mut self,
        call: &mut ToolCall,
        ctx: &PluginCtx,
    ) -> Result<Decision, PluginError> {
        if !self.seams.has(Seam::BeforeTool) {
            return Ok(Decision::Allow);
        }
        self.run.before_tool(call, ctx).await
    }

    async fn after_tool_result(
        &mut self,
        view: &ToolResultView<'_>,
        output: &mut ToolOutput,
        ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        if !self.seams.has(Seam::AfterTool) {
            return Ok(());
        }
        self.run.after_tool_result(view, output, ctx).await
    }

    async fn on_event(&mut self, event: &RunEvent, ctx: &PluginCtx) {
        if self.seams.has(Seam::OnEvent) {
            self.run.on_event(event, ctx).await;
        }
    }

    async fn before_request(
        &mut self,
        view: &RequestView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<ReasoningEffort>, PluginError> {
        if !self.seams.has(Seam::BeforeRequest) {
            return Ok(None);
        }
        self.run.before_request(view, ctx).await
    }

    async fn rewrite_context(
        &mut self,
        view: &ContextView<'_>,
        ctx: &PluginCtx,
    ) -> Result<Option<Rewrite>, PluginError> {
        if !self.seams.has(Seam::Rewrite) {
            return Ok(None);
        }
        self.run.rewrite_context(view, ctx).await
    }

    async fn rewritten(
        &mut self,
        replaced: &[Message],
        rewrite: &Rewrite,
        ctx: &PluginCtx,
    ) -> Result<(), PluginError> {
        if !self.seams.has(Seam::Rewritten) {
            return Ok(());
        }
        self.run.rewritten(replaced, rewrite, ctx).await
    }

    async fn before_stop(
        &mut self,
        message: &AssistantMessage,
        ctx: &PluginCtx,
    ) -> Result<StopDecision, PluginError> {
        if !self.seams.has(Seam::BeforeStop) {
            return Ok(StopDecision::Stop);
        }
        self.run.before_stop(message, ctx).await
    }

    async fn finish(&mut self, run: &FinishedRun<'_>, ctx: &PluginCtx) {
        if self.seams.has(Seam::Finish) {
            self.run.finish(run, ctx).await;
        }
    }
}
