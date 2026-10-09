//! tau-watcher's settings (ADR 0029): whether it is on, and the model
//! its side requests go to.

use gpui::{AnyElement, SharedString, div, prelude::*, rems};
use serde::{Deserialize, Serialize};
use tau_ai::model::{PLAN_FAMILIES, family_version, plan_models};
use tau_ui_kit::{
    components::{self as ui, Material as _},
    theme::{Design as _, Type, radius, sp},
};
use tau_ui_plugin::ViewCx;

use super::WatcherUi;

/// What the user set.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Off until switched on: each note costs a request.
    pub enabled: bool,
    /// One of [`PLAN_FAMILIES`]: the newest model of it reads the run.
    /// None is the chat's own model.
    pub family: Option<String>,
}

impl Settings {
    /// The model that reads the run; none for each chat's own. A family
    /// the plan does not offer falls back on the chat's model.
    pub fn model(&self) -> Option<String> {
        let family = self.family.as_deref()?;
        plan_models()
            .into_iter()
            .find(|model| {
                family_version(&model.id)
                    .is_some_and(|(name, _)| name == family)
            })
            .map(|model| model.id.clone())
    }

    /// What the pane says the watcher reads with.
    pub fn runs_as(&self) -> String {
        match (&self.family, self.model()) {
            (_, Some(model)) => format!("Reads with {model}."),
            (None, None) => "Reads with each chat's model.".to_owned(),
            (Some(family), None) => format!(
                "No {family} model on the plan: reads with each chat's model."
            ),
        }
    }
}

/// Its settings pane: the switch, and the model.
pub fn pane(view: &mut ViewCx<'_, WatcherUi>) -> AnyElement {
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
    let models = std::iter::once((None, "Chat's"))
        .chain(PLAN_FAMILIES.iter().map(|f| (Some(*f), *f)))
        .map(|(family, label)| {
            segment(
                format!("watcher-model-{label}"),
                label,
                settings.family.as_deref() == family,
                Settings {
                    family: family.map(str::to_owned),
                    ..settings.clone()
                },
            )
        })
        .collect::<Vec<_>>();
    let enabled = {
        let handle = view.handle.clone();
        let next = Settings {
            enabled: !settings.enabled,
            ..settings.clone()
        };
        let scope = scope.clone();
        div()
            .id("watcher-enabled")
            .cursor_pointer()
            .child(ui::switch(settings.enabled, &t))
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
                            "Note what I missed",
                            "Every 6th step, a side request reads the whole chat and \
                             almost always finds nothing. Now and then it offers one \
                             short note. Each check costs a request.",
                        ))
                        .child(enabled),
                )
                .child(
                    row(true)
                        .child(what(
                            "Model",
                            "Reads the chat and writes the notes. Chat's uses \
                             whatever model the chat runs on.",
                        ))
                        .child(
                            div()
                                .flex()
                                .gap(sp(0.5))
                                .p(sp(0.5))
                                .rounded(radius::BOX)
                                .well(&t)
                                .children(models),
                        ),
                ),
        )
        .child(ui::text(settings.runs_as(), Type::CAPTION.mono(), t.dim))
        .into_any_element()
}
