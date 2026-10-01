//! The extension points `tau-ui` declares, and the context each gives
//! its contributions. A plugin contributes to them in its manifest.

use tau_ui_kit::theme::Tone;

use crate::{
    manifest::{Point, PointCx},
    view::{NavEntry, PlanField, PluginStatus, RowNote, RunInfo},
};

/// No context: the app as a whole.
#[derive(Debug, Clone, Default)]
pub struct AtApp;

impl PointCx for AtApp {}

/// A repository.
#[derive(Debug, Clone)]
pub struct AtRepo {
    pub repo: String,
}

impl PointCx for AtRepo {}

/// A run.
#[derive(Debug, Clone)]
pub struct AtRun {
    pub run: RunInfo,
}

impl PointCx for AtRun {
    fn run(&self) -> Option<&RunInfo> {
        Some(&self.run)
    }
}

/// A message the person sent, in a run's transcript.
#[derive(Debug, Clone)]
pub struct AtMessage {
    pub run: RunInfo,
    pub text: String,
    /// Its place among the transcript's items, for element ids.
    pub index: usize,
}

impl PointCx for AtMessage {
    fn run(&self) -> Option<&RunInfo> {
        Some(&self.run)
    }
}

/// One of the plugin's anchors in a run's transcript.
#[derive(Debug, Clone)]
pub struct AtAnchor {
    pub run: RunInfo,
    pub key: String,
    /// The anchor's place among the transcript's items, for element ids.
    pub index: usize,
}

impl PointCx for AtAnchor {
    fn run(&self) -> Option<&RunInfo> {
        Some(&self.run)
    }
}

/// One of the plugin's context rewrites in a run's transcript.
#[derive(Debug, Clone)]
pub struct AtRewrite {
    pub run: RunInfo,
    /// What the plugin named it ([`crate::RunCx::rewrite`]).
    pub key: String,
    /// The context's tokens before and after it, when the run saw it.
    pub tokens: Option<(u64, u64)>,
    /// Its place among the transcript's items, for element ids.
    pub index: usize,
}

impl PointCx for AtRewrite {
    fn run(&self) -> Option<&RunInfo> {
        Some(&self.run)
    }
}

/// A tool call's card, with the anchors the plugin attached to it.
#[derive(Debug, Clone)]
pub struct AtCard {
    pub run: RunInfo,
    pub call_id: String,
    pub tool: String,
    /// The plugin's anchors on the card, in the order they came.
    pub keys: Vec<String>,
    /// What the call sent and returned. Shared: a card's points share it.
    pub data: std::sync::Arc<crate::CallData>,
    /// The argument worth reading at a glance.
    pub summary: String,
    /// What a plugin cut from its result, when one did.
    pub cut: Option<crate::OutputCut>,
}

/// How a tool's own plugin draws its card, in tau-ui's frame.
#[derive(Default)]
pub struct CardView {
    /// In place of the summary of its arguments.
    pub head: Option<gpui::AnyElement>,
    /// The result's line at the header's end: `+12 −3`, `3 matches`.
    pub label: Option<String>,
    /// The call failed, as its result tells, though the tool returned.
    pub failed: Option<String>,
    /// The card's edge, when its result wants attention.
    pub edge: Option<Tone>,
    /// A small drawing of the result, before the fold's chevron.
    pub shape: Option<gpui::AnyElement>,
    /// Under the header.
    pub body: Option<gpui::AnyElement>,
    /// Whether the body folds, closed until its header is clicked.
    pub folds: bool,
    /// Whether the body sits inset under the header and says itself what
    /// was cut from the result (a terminal's screen).
    pub inset: bool,
}

impl PointCx for AtCard {
    fn run(&self) -> Option<&RunInfo> {
        Some(&self.run)
    }
}

/// An item in a run's transcript, at a plugin's anchor.
pub const TRANSCRIPT: Point<AtAnchor> = Point::new("tau.run.transcript");
/// One of the plugin's context rewrites, drawn whole.
pub const REWRITE: Point<AtRewrite> = Point::new("tau.run.rewrite");
/// Under the inspector's context meter.
pub const CONTEXT: Point<AtRun> = Point::new("tau.run.context");
/// A tool call's card, drawn by the plugin whose tool it is: the first
/// to answer draws it.
pub const CARD: Point<AtCard, CardView> = Point::new("tau.run.card");
/// Beside a tool card's title.
pub const CARD_BADGE: Point<AtCard> = Point::new("tau.run.card.badge");
/// Under a tool card's body.
pub const CARD_BODY: Point<AtCard> = Point::new("tau.run.card.body");
/// Above a run's transcript.
pub const RUN_BANNER: Point<AtRun> = Point::new("tau.run.banner");
/// A section of the run inspector.
pub const INSPECTOR: Point<AtRun> = Point::new("tau.run.inspector");
/// The plugin's line in the run's plugin list.
pub const STATUS: Point<AtRun, PluginStatus> = Point::new("tau.run.status");
/// A field of the run's plan.
pub const PLAN: Point<AtRun, PlanField> = Point::new("tau.run.plan");
/// Where on the context meter the plugin steps in, as a share of the
/// window.
pub const CONTEXT_TRIGGER: Point<AtRun, f32> =
    Point::new("tau.run.context.trigger");
/// A line and small print on a run's row in the sidebar: a goal's.
pub const RUN_ROW: Point<AtRun, RowNote> = Point::new("tau.sidebar.run.row");
/// A message the person sent, drawn by the plugin that reads it as its
/// own (a `/goal`); the first contribution draws it.
pub const USER_MESSAGE: Point<AtMessage> = Point::new("tau.run.user_message");
/// The sidebar's own entries, above the repositories.
pub const SIDEBAR: Point<AtApp, NavEntry> = Point::new("tau.sidebar");
/// Entries under each repository in the sidebar and the phone's Runs
/// list.
pub const SIDEBAR_REPO: Point<AtRepo, NavEntry> =
    Point::new("tau.sidebar.repo");
/// A section of the Models screen.
pub const MODELS: Point<AtApp> = Point::new("tau.models.section");
/// Hits search offers besides the navigation's entries.
pub const SEARCH: Point<AtApp, NavEntry> = Point::new("tau.search");
/// A step on a run's Plan screen: what the plugin's `start` decided.
pub const PLAN_STEPS: Point<AtRun> = Point::new("tau.run.plan.steps");
/// What the model picker says auto does, under its efforts.
pub const PICKER_AUTO: Point<AtApp, String> = Point::new("tau.picker.auto");

/// Every point `tau-ui` declares.
pub const ALL: [&str; 19] = [
    CARD.name,
    TRANSCRIPT.name,
    REWRITE.name,
    CONTEXT.name,
    CARD_BADGE.name,
    CARD_BODY.name,
    RUN_BANNER.name,
    INSPECTOR.name,
    STATUS.name,
    PLAN.name,
    CONTEXT_TRIGGER.name,
    RUN_ROW.name,
    USER_MESSAGE.name,
    SIDEBAR.name,
    SIDEBAR_REPO.name,
    MODELS.name,
    SEARCH.name,
    PLAN_STEPS.name,
    PICKER_AUTO.name,
];
