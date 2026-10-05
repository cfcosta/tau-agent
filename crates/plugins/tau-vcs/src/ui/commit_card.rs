//! The `vcs_commit` card, drawn as the stack the commit made: the
//! parent below, the new change in the middle, and the working copy on
//! top. Closed, a small graph and the change's subject; open, each node
//! with its files, so a commit that took only some paths shows what
//! went into the change and what stayed in `@`.

use gpui::{Div, div, prelude::*, rems};
use serde::Deserialize;
use serde_json::Value;
use tau_ui_kit::{
    components::mono,
    theme::{Design as _, Theme, Type, sp, weight},
};

use super::{
    Card,
    change_diff::{FileDiff, files_of},
    change_log::Change,
    diff_card::{file_list, kind_badge},
    log_card::{bookmarks, short_id},
};
use crate::{ChangeInfo, FileChange};

/// What a `vcs_commit` result shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The change the commit made.
    pub change: Change,
    /// The change it sits on, when it has one.
    pub parent: Option<Change>,
    /// The new `@` on top of it.
    pub working_copy: Change,
    /// What the change holds against its parent.
    pub files: Vec<FileDiff>,
    pub truncated: bool,
    /// What stays in `@`, uncommitted.
    pub left: Vec<FileDiff>,
    pub left_truncated: bool,
}

#[derive(Deserialize)]
struct Details {
    committed: ChangeInfo,
    working_copy: ChangeInfo,
    parent: Option<ChangeInfo>,
    files: Vec<FileChange>,
    diff: String,
    truncated: bool,
    #[serde(default)]
    left_files: Vec<FileChange>,
    #[serde(default)]
    left_diff: String,
    #[serde(default)]
    left_truncated: bool,
}

impl Commit {
    /// Reads a `vcs_commit` result's details. `None` when they are not
    /// that shape.
    pub fn parse(details: &Value) -> Option<Self> {
        let details = Details::deserialize(details).ok()?;
        Some(Self {
            change: Change::new(details.committed),
            parent: details.parent.map(Change::new),
            working_copy: Change::new(details.working_copy),
            files: files_of(&details.diff, &details.files, details.truncated),
            truncated: details.truncated,
            left: files_of(
                &details.left_diff,
                &details.left_files,
                details.left_truncated,
            ),
            left_truncated: details.left_truncated,
        })
    }

    /// The closed card's count: `2 files`, or `1 of 3 files committed`
    /// when some stayed in `@`.
    pub fn label(&self) -> String {
        let files = |count: usize| match count {
            1 => "1 file".to_owned(),
            count => format!("{count} files"),
        };
        if self.left.is_empty() {
            files(self.files.len())
        } else {
            format!(
                "{} of {} committed",
                self.files.len(),
                files(self.files.len() + self.left.len())
            )
        }
    }
}

fn subject(change: &Change) -> String {
    if change.subject.is_empty() {
        "(no description set)".into()
    } else {
        change.subject.clone()
    }
}

/// A round node of the graph, `size` rem across.
fn node(size: f32) -> Div {
    div().size(rems(size)).flex_shrink_0().rounded_full()
}

/// The working copy's node: a dashed ring.
fn working_node(size: f32, t: &Theme) -> Div {
    node(size).border_1().border_dashed().border_color(t.dim)
}

/// The new change's node: filled in the change color, with a halo.
fn change_node(size: f32, t: &Theme) -> Div {
    div()
        .size(rems(size + 0.5))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.change.opacity(0.15))
        .child(node(size).bg(t.change))
}

/// The parent's node: a quiet ring.
fn parent_node(size: f32, t: &Theme) -> Div {
    node(size).border_1().border_color(t.slate)
}

/// The closed card's head: the graph in small, the change's type and
/// subject, and the bookmarks on it.
pub fn summary(commit: &Commit, t: &Theme) -> Div {
    let link = || div().w(rems(0.875)).h(rems(0.09375)).bg(t.border_strong);
    let change = &commit.change;
    div()
        .flex_1()
        .min_w(rems(0.))
        .flex()
        .items_center()
        .gap(sp(2.))
        .child(
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap(sp(1.))
                .when(commit.parent.is_some(), |graph| {
                    graph.child(parent_node(0.4375, t)).child(link())
                })
                .child(node(0.5625).bg(t.change))
                .child(link())
                .child(working_node(0.4375, t)),
        )
        .children(change.kind.as_ref().map(|_| kind_badge(change, t)))
        .child(
            div()
                .flex_1()
                .min_w(rems(0.))
                .truncate()
                .typeset(Type::SMALL)
                .text_color(t.text_soft)
                .child(subject(change)),
        )
        .children(bookmarks(change, t))
}

/// One node of the open graph: its mark in the rail, the line on to the
/// node below unless it is the last, and what it holds.
fn row(mark: Div, last: bool, content: Div, t: &Theme) -> Div {
    div()
        .flex()
        .gap(sp(3.5))
        .child(
            div()
                .w(rems(1.125))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .child(div().pt(sp(1.)).child(mark))
                .when(!last, |rail| {
                    rail.child(
                        div().flex_1().w(rems(0.09375)).bg(t.border_strong),
                    )
                }),
        )
        .child(
            content
                .flex_1()
                .min_w(rems(0.))
                .when(!last, |content| content.pb(sp(4.))),
        )
}

/// The open card: `@` on top with what stayed in it, the new change
/// with its message and files, and the parent below.
pub fn body(card: &Card<'_>, commit: &Commit, t: &Theme, compact: bool) -> Div {
    let change = &commit.change;
    let left = commit.left.len();
    let working = div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .child(
                    mono("@", Type::CAPTION, t.text)
                        .font_weight(weight::EMPHASIS),
                )
                .child(short_id(&commit.working_copy, t))
                .child(div().typeset(Type::SMALL).text_color(t.dim).child(
                    match left {
                        0 => "working copy · empty".to_owned(),
                        1 => "working copy · still holds 1 file".to_owned(),
                        n => format!("working copy · still holds {n} files"),
                    },
                )),
        )
        .when(left > 0, |column| {
            column.child(file_list(
                card,
                &commit.left,
                commit.left_truncated,
                &|_| false,
                t,
                compact,
            ))
        });
    let message = change
        .info
        .description
        .trim()
        .split_once('\n')
        .map(|(_, body)| body.trim().to_owned())
        .filter(|body| !body.is_empty());
    let committed = div()
        .flex()
        .flex_col()
        .gap(sp(2.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.))
                .children(change.kind.as_ref().map(|_| kind_badge(change, t)))
                .child(
                    div()
                        .flex_1()
                        .min_w(rems(0.))
                        .typeset(Type::BODY)
                        .font_weight(weight::EMPHASIS)
                        .text_color(if change.subject.is_empty() {
                            t.dim
                        } else {
                            t.text
                        })
                        .child(subject(change)),
                )
                .child(short_id(change, t))
                .children(bookmarks(change, t)),
        )
        .children(message.map(|message| {
            div().typeset(Type::SMALL).text_color(t.muted).children(
                message.lines().map(|line| {
                    // A blank line between paragraphs keeps its height.
                    div().min_h(rems(1.25)).child(line.to_owned())
                }),
            )
        }))
        .child(
            div()
                .py(sp(1.))
                .rounded(tau_ui_kit::theme::radius::BOX)
                .bg(t.bg)
                .border_1()
                .border_color(t.border)
                .child(file_list(
                    card,
                    &commit.files,
                    commit.truncated,
                    &|_| false,
                    t,
                    compact,
                )),
        );
    let parent = commit.parent.as_ref().map(|parent| {
        div()
            .flex()
            .items_center()
            .gap(sp(2.))
            .h(rems(1.25))
            .child(mono(parent.short_id().to_owned(), Type::CAPTION, t.dim))
            .children(bookmarks(parent, t))
            .child(
                div()
                    .min_w(rems(0.))
                    .truncate()
                    .typeset(Type::SMALL)
                    .text_color(t.dim)
                    .child(parent.info.description.lines().next().map_or_else(
                        || "(no description set)".to_owned(),
                        str::to_owned,
                    )),
            )
    });
    let has_parent = parent.is_some();
    div()
        .flex()
        .flex_col()
        .px(sp(4.))
        .pt(sp(3.5))
        .pb(sp(4.))
        .border_t_1()
        .border_color(t.border)
        .child(row(working_node(0.75, t), false, working, t))
        .child(row(change_node(0.875, t), !has_parent, committed, t))
        .children(
            parent.map(|parent| row(parent_node(0.625, t), true, parent, t)),
        )
}
