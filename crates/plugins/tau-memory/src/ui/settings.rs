//! tau-memory's settings (ADR 0029): the model it writes notes with,
//! before compaction drops a conversation and after a run, and how hard
//! that model reasons. Search runs locally and needs none.

use gpui::{AnyElement, SharedString, div, prelude::*, rems};
use serde::{Deserialize, Serialize};
use tau_ai::{
    model::{PLAN_FAMILIES, family_version, plan_models},
    responses::request::ReasoningEffort,
};
use tau_ui_kit::{
    components::{self as ui, Material as _},
    theme::{Design as _, Type, radius, sp},
};
use tau_ui_plugin::ViewCx;

use super::MemoryUi;

/// The family memory writes with unless the user picks another: the
/// newest of it on the plan.
pub const DEFAULT_FAMILY: &str = "luna";

/// The effort memory writes with unless the user picks another.
pub const DEFAULT_REASONING: &str = "low";

/// The efforts the pane offers; `None` leaves it to the model.
pub const EFFORTS: [Option<&str>; 4] =
    [None, Some("low"), Some("medium"), Some("high")];

/// What the model memory writes with is, as the user set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// One of [`PLAN_FAMILIES`]: the newest model of it writes.
    pub family: String,
    /// The effort, as [`ReasoningEffort::as_str`] spells it; `None`
    /// leaves it to the model.
    pub reasoning: Option<String>,
    /// Writes with the run's own model and effort instead.
    pub follow_chat: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            family: DEFAULT_FAMILY.to_owned(),
            reasoning: Some(DEFAULT_REASONING.to_owned()),
            follow_chat: false,
        }
    }
}

impl Settings {
    /// The model memory writes with and its effort; `None` for the
    /// run's own. A family the plan does not offer falls back on the
    /// run's model, and an effort the model does not take on the
    /// model's default.
    pub fn model(&self) -> Option<(String, Option<ReasoningEffort>)> {
        if self.follow_chat {
            return None;
        }
        let model = plan_models().into_iter().find(|model| {
            family_version(&model.id).is_some_and(|(family, _)| family == self.family)
        })?;
        let reasoning = self
            .reasoning
            .as_deref()
            .and_then(ReasoningEffort::parse)
            .filter(|effort| model.efforts.contains(effort));
        Some((model.id.clone(), reasoning))
    }

    /// What the pane says memory runs as: `gpt-6-luna · low`, or the
    /// chat's model.
    pub fn runs_as(&self) -> String {
        match self.model() {
            Some((model, effort)) => format!(
                "Runs as {model} · {}.",
                effort.map_or("auto", ReasoningEffort::as_str)
            ),
            None if self.follow_chat => "Runs as each chat's model.".to_owned(),
            None => format!(
                "No {} model on the plan: runs as each chat's model.",
                self.family
            ),
        }
    }
}

/// Its settings pane: the family, the effort, and the switch to write
/// with the chat's model instead.
pub fn pane(view: &mut ViewCx<'_, MemoryUi>) -> AnyElement {
    let t = view.theme().clone();
    let settings = view.settings.clone();
    let scope = view.scope().map(str::to_owned);
    let row = |last: bool| {
        div()
            .flex()
            .items_center()
            .gap(sp(4.))
            .px(sp(4.))
            .py(sp(3.5))
            .when(!last, |row| row.border_b_1().border_color(t.border))
    };
    let what = |name: &str, caption: &str| {
        div()
            .flex_1()
            .min_w(rems(0.))
            .flex()
            .flex_col()
            .gap(sp(0.75))
            .child(name.to_owned())
            .child(ui::text(caption.to_owned(), Type::CAPTION, t.muted))
    };
    // One choice of a segmented control: `on` when it is the setting,
    // saving `next` when clicked.
    let segment = |id: String, label: &str, on: bool, next: Settings| {
        let handle = view.handle.clone();
        let scope = scope.clone();
        div()
            .id(SharedString::from(id))
            .h(rems(1.75))
            .px(sp(2.5))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius::CONTROL)
            .typeset(Type::CAPTION)
            .cursor_pointer()
            .text_color(if on { t.text } else { t.muted })
            .when(on, |segment| segment.key(&t))
            .child(label.to_owned())
            .on_click(move |_, _, cx| {
                handle.save_settings_in(scope.as_deref(), &next, cx)
            })
    };
    let control = |segments: Vec<gpui::Stateful<gpui::Div>>| {
        div()
            .flex()
            .gap(sp(0.5))
            .p(sp(0.5))
            .rounded(radius::BOX)
            .well(&t)
            .children(segments)
    };
    let families = PLAN_FAMILIES
        .iter()
        .map(|family| {
            segment(
                format!("memory-family-{family}"),
                family,
                settings.family == *family,
                Settings {
                    family: (*family).to_owned(),
                    ..settings.clone()
                },
            )
        })
        .collect();
    let efforts = EFFORTS
        .iter()
        .map(|effort| {
            segment(
                format!("memory-effort-{}", effort.unwrap_or("auto")),
                effort.unwrap_or("auto"),
                settings.reasoning.as_deref() == *effort,
                Settings {
                    reasoning: effort.map(str::to_owned),
                    ..settings.clone()
                },
            )
        })
        .collect();
    let follow = {
        let handle = view.handle.clone();
        let scope = scope.clone();
        let next = Settings {
            follow_chat: !settings.follow_chat,
            ..settings.clone()
        };
        div()
            .id("memory-follow-chat")
            .cursor_pointer()
            .child(ui::switch(settings.follow_chat, &t))
            .on_click(move |_, _, cx| {
                handle.save_settings_in(scope.as_deref(), &next, cx)
            })
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(
            ui::card(&t)
                .child(
                    row(false)
                        .child(what(
                            "Model",
                            "Writes notes before compaction drops the conversation, and \
                             after a run when consolidation is on. Search runs locally \
                             and needs none.",
                        ))
                        .child(control(families)),
                )
                .child(
                    row(false)
                        .child(what(
                            "Reasoning",
                            "How hard it thinks about what is worth keeping. Auto leaves \
                             it to the model.",
                        ))
                        .child(control(efforts)),
                )
                .child(
                    row(true)
                        .child(what(
                            "Use the chat's model instead",
                            "Notes come from whatever model and effort the run is on.",
                        ))
                        .child(follow),
                ),
        )
        .child(ui::text(settings.runs_as(), Type::CAPTION.mono(), t.dim))
        .into_any_element()
}
