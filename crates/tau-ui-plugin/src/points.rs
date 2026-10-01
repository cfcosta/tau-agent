//! The extension points `tau-ui` declares, and the context each gives
//! its contributions. A plugin contributes to them in its manifest.

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

/// A tool call's card, with the anchors the plugin attached to it.
#[derive(Debug, Clone)]
pub struct AtCard {
    pub run: RunInfo,
    pub call_id: String,
    pub tool: String,
    /// The plugin's anchors on the card, in the order they came.
    pub keys: Vec<String>,
}

impl PointCx for AtCard {
    fn run(&self) -> Option<&RunInfo> {
        Some(&self.run)
    }
}

/// An item in a run's transcript, at a plugin's anchor.
pub const TRANSCRIPT: Point<AtAnchor> = Point::new("tau.run.transcript");
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
pub const ALL: [&str; 16] = [
    TRANSCRIPT.name,
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
