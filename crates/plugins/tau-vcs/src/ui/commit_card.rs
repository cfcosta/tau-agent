//! A `vcs_commit` card: the change it made, its message's body, and the
//! paths it left in the working copy when it committed only some.

use gpui::{Div, div, prelude::*};
use tau_ui_kit::{
    prose::prose,
    theme::{Design as _, Theme, Type, sp},
};

use super::{change_log::Change, log_card::stack_rows};
use crate::ChangeInfo;

/// The card's body: the committed change as a stack row, the rest of
/// its message, and what stayed uncommitted.
pub fn body(committed: &ChangeInfo, left: &[String], t: &Theme) -> Div {
    let message = committed.description.trim_end();
    let rest = message
        .split_once('\n')
        .map(|(_, rest)| rest.trim())
        .filter(|rest| !rest.is_empty());
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_col()
                .py(sp(1.5))
                .border_t_1()
                .border_color(t.border)
                .children(stack_rows(
                    &[Change::new(committed.clone())],
                    t,
                    false,
                )),
        )
        .when_some(rest, |column, rest| {
            column.child(
                div()
                    .px(sp(3.75))
                    .pb(sp(2.))
                    .typeset(Type::CAPTION)
                    .text_color(t.muted)
                    .child(prose(rest, t.muted, t)),
            )
        })
        .when(!left.is_empty(), |column| {
            column.child(
                div()
                    .px(sp(3.75))
                    .pb(sp(2.))
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(prose(
                        &format!(
                            "Left uncommitted: {}",
                            left.iter()
                                .map(|path| format!("`{path}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        t.dim,
                        t,
                    )),
            )
        })
}
