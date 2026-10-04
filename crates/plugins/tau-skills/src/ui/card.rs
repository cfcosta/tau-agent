//! The card of a `skill` call: the skill's name and what it is for,
//! then, folded, its file and the instructions the model read, from the
//! call's own arguments and result, so a stored run shows it as the live
//! one did.

use gpui::{div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{icon, mono, text},
    theme::{Design as _, IconSize, Tone, Type, sp},
};
use tau_ui_plugin::{
    ViewCx,
    points::{AtCard, CardView},
};

use super::SkillsUi;
use crate::{Loaded, TOOL};

/// The card of a call to `skill`.
pub fn card(at: &AtCard, view: &mut ViewCx<'_, SkillsUi>) -> Option<CardView> {
    if at.tool != TOOL {
        return None;
    }
    let t = view.theme().clone();
    let color = t.roles.tool_run;
    let name = at.data.args["name"].as_str().unwrap_or_default().to_owned();
    let result = at.data.result.as_ref();
    let loaded: Option<Loaded> = result
        .filter(|result| !result.error)
        .and_then(|result| result.details.clone())
        .and_then(|details| serde_json::from_value(details).ok());
    let failed = result.filter(|result| result.error).map(|result| {
        result.text.lines().next().unwrap_or_default().to_owned()
    });
    let description = loaded.as_ref().map(|loaded| loaded.description.clone());
    let head = div()
        .flex()
        .items_center()
        .gap(sp(2.))
        .min_w(rems(0.))
        .child(icon(Icon::Skill, IconSize::COMPACT, color))
        .child(
            mono(
                name,
                Type::CAPTION,
                if failed.is_some() { t.red } else { color },
            )
            .flex_shrink_0(),
        )
        .children(description.map(|description| {
            text(description, Type::CAPTION, t.dim)
                .min_w(rems(0.))
                .truncate()
        }));
    // What the model read: the result after its line naming the folder.
    let instructions = result
        .filter(|result| !result.error)
        .and_then(|result| result.text.split_once("\n\n"))
        .map(|(_, body)| body.to_owned());
    let body = loaded.map(|loaded| {
        div()
            .flex()
            .flex_col()
            .gap(sp(2.))
            .child(mono(
                loaded.file.display().to_string(),
                Type::CAPTION,
                t.dim,
            ))
            .children(instructions.map(|body| {
                text(body, Type::SMALL, t.text)
                    .leading(1.6)
                    .whitespace_normal()
            }))
            .into_any_element()
    });
    Some(CardView {
        head: Some(head.into_any_element()),
        label: None,
        edge: failed.as_ref().map(|_| Tone::Danger),
        failed,
        shape: None,
        body,
        folds: true,
        inset: false,
    })
}
