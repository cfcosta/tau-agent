//! The Skills screen: each skill in the person's folder, with what it
//! is for, and each folder that is not one, with why.

use gpui::{AnyElement, Div, div, prelude::*, rems};
use tau_ui_kit::{
    assets::Icon,
    components::{self as ui, ButtonKind, icon, mono, text},
    theme::{Design as _, IconSize, Theme, Type, radius, sp},
};
use tau_ui_plugin::ViewCx;

use super::SkillsUi;
use crate::{Problem, SKILL_FILE, Skill, Skills};

/// The screen's column, as other plugins' screens have it.
const COLUMN: f32 = 860.;

pub fn title(_: &mut ViewCx<'_, SkillsUi>) -> String {
    "Skills".to_owned()
}

pub fn render(view: &mut ViewCx<'_, SkillsUi>) -> AnyElement {
    let t = view.theme().clone();
    let skills = view.data.clone();
    let column = div()
        .w_full()
        .max_w(rems(COLUMN / 16.))
        .flex()
        .flex_col()
        .gap(sp(5.))
        .child(header(&skills, &t))
        .child(list(&skills, &t));
    ui::screen(
        "skills",
        view.compact,
        div().flex().justify_center().child(column),
    )
    .into_any_element()
}

fn header(skills: &Skills, t: &Theme) -> Div {
    let title = ui::screen_title(
        "Skills",
        format!(
            "Instructions the model loads when a task calls for them, or \
             that you start with /name. Each is a folder with a {SKILL_FILE}."
        ),
        t,
    );
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_end()
        .gap(sp(4.))
        .child(title);
    if let Some(dir) = skills.dir.clone() {
        let open = div()
            .id("skills-open-folder")
            .ml_auto()
            .child(ui::button("Open folder", ButtonKind::Secondary, t))
            .on_click(move |_, _, cx| cx.reveal_path(&dir));
        row = row.child(open);
    }
    let from = skills.dir.as_ref().map(|dir| {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(sp(2.))
            .child(text("From", Type::SMALL, t.text_soft))
            .child(
                mono(dir.display().to_string(), Type::CAPTION, t.text)
                    .px(sp(2.))
                    .py(sp(0.75))
                    .rounded(radius::CONTROL)
                    .bg(t.panel)
                    .border_1()
                    .border_color(t.border),
            )
            .child(text(
                "read as each run starts; new ones show in the next run",
                Type::SMALL,
                t.dim,
            ))
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(3.))
        .child(row)
        .children(from)
}

fn list(skills: &Skills, t: &Theme) -> Div {
    if skills.found.is_empty() && skills.problems.is_empty() {
        return ui::empty(
            format!(
                "No skills yet. A skill is a folder here with a {SKILL_FILE}: \
                 YAML frontmatter with a name and a description, then the \
                 instructions."
            ),
            t,
        );
    }
    div()
        .flex()
        .flex_col()
        .rounded(radius::CARD)
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        .children(skills.found.iter().map(|skill| row(skill, t)))
        .children(
            skills
                .problems
                .iter()
                .map(|problem| problem_row(problem, t)),
        )
}

fn row(skill: &Skill, t: &Theme) -> Div {
    let files = match skill.files {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let ignored = (!skill.ignored.is_empty()).then(|| {
        text(
            format!(
                "Also sets {}, which tau does not act on yet.",
                skill.ignored.join(", ")
            ),
            Type::CAPTION,
            t.dim,
        )
    });
    div()
        .flex()
        .gap(sp(4.))
        .px(sp(4.))
        .py(sp(3.5))
        .border_b_1()
        .border_color(t.border)
        .bg(t.panel)
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .child(mono(skill.name.clone(), Type::BODY, t.roles.tool_run))
                .child(
                    text(skill.description.clone(), Type::BODY, t.text_soft)
                        .leading(1.5),
                )
                .children(ignored),
        )
        .child(mono(files, Type::CAPTION, t.dim).flex_shrink_0())
}

fn problem_row(problem: &Problem, t: &Theme) -> Div {
    let name = problem
        .dir
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    div()
        .flex()
        .gap(sp(4.))
        .px(sp(4.))
        .py(sp(3.5))
        .border_b_1()
        .border_color(t.border)
        .bg(t.danger_surface)
        .child(icon(Icon::Warning, IconSize::BASE, t.red).flex_shrink_0())
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .flex()
                .flex_col()
                .gap(sp(1.))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(sp(2.5))
                        .child(mono(name, Type::BODY, t.removed_text))
                        .child(text(
                            "not offered to the model",
                            Type::CAPTION,
                            t.removed_text,
                        )),
                )
                .child(
                    text(problem.reason.clone(), Type::BODY, t.text)
                        .leading(1.5),
                )
                .child(mono(
                    problem.dir.display().to_string(),
                    Type::CAPTION,
                    t.dim,
                )),
        )
}
