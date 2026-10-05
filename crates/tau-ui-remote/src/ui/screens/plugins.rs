//! The agent's plugins (ADR 0029): a list grouped by what each plugin is
//! for, and a pane for the one selected, with its settings, what it did
//! and how it works. On a phone the list and the pane are two steps.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};
use tau_ui_plugin::{CatalogEntry, Group, Note, Seam};

use crate::{
    catalog::PluginInfo,
    route::Route,
    theme::{Design as _, Theme, Type, radius, sp},
    ui::{self, Material as _, mono},
    view::{tokens, usd},
    workspace::Workspace,
};

/// The pane's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Settings,
    Work,
    About,
}

impl Tab {
    const ALL: [Self; 3] = [Self::Settings, Self::Work, Self::About];

    fn label(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Work => "What it did",
            Self::About => "How it works",
        }
    }
}

/// What the Plugins screen shows: the plugin selected (and its entry,
/// for one that lists its own), the pane's tab, and the scope its
/// settings show, a repository or everywhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub plugin: Option<String>,
    pub entry: Option<String>,
    pub tab: Tab,
    pub scope: Option<String>,
}

/// A row of the list: a plugin, or one of its entries.
struct Row<'a> {
    plugin: &'a PluginInfo,
    entry: Option<&'a CatalogEntry>,
}

impl Row<'_> {
    fn name(&self) -> &str {
        self.entry.map_or(&self.plugin.name, |entry| &entry.name)
    }

    fn description(&self) -> &str {
        self.entry
            .map_or(&self.plugin.description, |entry| &entry.description)
    }

    fn group(&self) -> Group {
        self.entry.map_or(self.plugin.group, |entry| entry.group)
    }

    fn note(&self) -> Option<&Note> {
        self.entry
            .map_or(self.plugin.note.as_ref(), |entry| entry.note.as_ref())
    }

    fn seams(&self) -> &[Seam] {
        self.entry.map_or(&self.plugin.seams, |entry| &entry.seams)
    }

    fn is(&self, selection: &Selection) -> bool {
        selection.plugin.as_deref() == Some(self.plugin.name.as_str())
            && selection.entry.as_deref() == self.entry.map(|e| e.name.as_str())
    }
}

/// Every row, in the catalog's order: each plugin, then its entries.
fn rows(ws: &Workspace) -> Vec<Row<'_>> {
    ws.catalog
        .plugins
        .iter()
        .flat_map(|plugin| {
            std::iter::once(Row {
                plugin,
                entry: None,
            })
            .chain(plugin.entries.iter().map(move |entry| Row {
                plugin,
                entry: Some(entry),
            }))
        })
        .collect()
}

impl Workspace {
    /// Selects `plugin` (and its `entry`) on the Plugins screen.
    pub fn pick_plugin(
        &mut self,
        plugin: &str,
        entry: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let screen = &mut self.plugins_screen;
        screen.plugin = Some(plugin.to_owned());
        screen.entry = entry.map(str::to_owned);
        cx.notify();
    }

    /// Back to the list, on a phone.
    pub fn unpick_plugin(&mut self, cx: &mut Context<Self>) {
        self.plugins_screen.plugin = None;
        self.plugins_screen.entry = None;
        cx.notify();
    }

    pub fn set_plugin_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        self.plugins_screen.tab = tab;
        cx.notify();
    }

    /// Shows the settings everywhere (`None`), or a repository's.
    pub fn set_plugin_scope(
        &mut self,
        scope: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.plugins_screen.scope = scope;
        cx.notify();
    }

    /// Opens `plugin`'s settings in the inspector, or closes them.
    pub fn toggle_inspector_plugin(
        &mut self,
        plugin: &str,
        cx: &mut Context<Self>,
    ) {
        self.inspector_plugin = match &self.inspector_plugin {
            Some(open) if open == plugin => None,
            _ => Some(plugin.to_owned()),
        };
        cx.notify();
    }

    /// The plugin and entry the screen shows: the one picked, else the
    /// first (on a computer; a phone starts on the list).
    fn shown_selection(&self) -> Option<(String, Option<String>)> {
        let screen = &self.plugins_screen;
        if let Some(plugin) = &screen.plugin
            && rows(self).iter().any(|row| row.is(screen))
        {
            return Some((plugin.clone(), screen.entry.clone()));
        }
        if self.compact() {
            return None;
        }
        self.catalog
            .plugins
            .first()
            .map(|plugin| (plugin.name.clone(), None))
    }
}

pub fn render(
    ws: &Workspace,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let shown = ws.shown_selection();
    let selection = Selection {
        plugin: shown.as_ref().map(|(plugin, _)| plugin.clone()),
        entry: shown.and_then(|(_, entry)| entry),
        ..ws.plugins_screen.clone()
    };
    if compact {
        return if selection.plugin.is_some() {
            ui::screen("plugins", true, pane(ws, &selection, true, t, cx))
                .into_any_element()
        } else {
            ui::screen("plugins", true, list(ws, &selection, t, cx))
                .into_any_element()
        };
    }
    div()
        .flex_1()
        .min_h(rems(0.))
        .flex()
        .child(
            div()
                .id("plugins-list")
                .w(rems(20.))
                .flex_shrink_0()
                .overflow_y_scroll()
                .border_r_1()
                .border_color(t.border)
                .bg(t.panel)
                .px(sp(3.))
                .py(sp(6.))
                .child(list(ws, &selection, t, cx)),
        )
        .child(
            div()
                .id("plugins-pane")
                .flex_1()
                .min_w(rems(0.))
                .overflow_y_scroll()
                .px(sp(10.))
                .py(sp(7.))
                .child(
                    div()
                        .max_w(rems(52.))
                        .child(pane(ws, &selection, false, t, cx)),
                ),
        )
        .into_any_element()
}

/// The list: a heading with what plugins charged, then each group's
/// rows, then Jev's line.
fn list(
    ws: &Workspace,
    selection: &Selection,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let catalog = &ws.catalog;
    let spent: f64 = catalog.plugins.iter().map(|plugin| plugin.spend).sum();
    let rows = rows(ws);
    let groups = Group::ALL.into_iter().filter_map(|group| {
        let rows: Vec<AnyElement> = rows
            .iter()
            .filter(|row| row.group() == group)
            .map(|row| list_row(row, row.is(selection), t, cx))
            .collect();
        (!rows.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(sp(0.5))
                .child(
                    div()
                        .px(sp(2.))
                        .pb(sp(1.5))
                        .typeset(Type::MICRO)
                        .text_color(t.dim)
                        .child(group.label().to_uppercase()),
                )
                .children(rows)
        })
    });
    let jev = catalog.jev.as_ref().map(|jev| {
        div()
            .px(sp(2.))
            .flex()
            .flex_wrap()
            .gap(sp(1.5))
            .typeset(Type::CAPTION)
            .text_color(t.dim)
            .child(format!("tau-jev on {}", jev.model))
            .child(format!(
                "· {} requests, {} tokens in",
                jev.requests,
                tokens(jev.input_tokens)
            ))
            .when(jev.failed > 0, |line| {
                line.child(
                    div()
                        .text_color(t.red)
                        .child(format!("· {} failed", jev.failed)),
                )
            })
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(
            div()
                .px(sp(2.))
                .flex()
                .items_baseline()
                .justify_between()
                .child(
                    div()
                        .typeset(Type::HEADING)
                        .text_color(t.text)
                        .child("Plugins"),
                )
                .child(mono(usd(spent), Type::CAPTION, t.roles.cost)),
        )
        .children(groups)
        .children(jev)
        .into_any_element()
}

fn list_row(
    row: &Row<'_>,
    selected: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let name = row.name().to_owned();
    let plugin = row.plugin.name.clone();
    let entry = row.entry.map(|entry| entry.name.clone());
    let ink = t.roles.plugin(&name).unwrap_or(t.mark(&name));
    div()
        .id(SharedString::from(format!("plugin-row-{plugin}-{name}")))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(rems(2.25))
        .px(sp(2.))
        .rounded(radius::CONTROL)
        .cursor_pointer()
        .when(selected, |row| row.bg(t.selected))
        .when(!selected, |row| row.hover(|style| style.bg(t.card)))
        .when(row.entry.is_some(), |row| row.pl(sp(4.)))
        .child(
            div()
                .size(rems(0.4375))
                .flex_shrink_0()
                .rounded(radius::FULL)
                .bg(ink),
        )
        .child(
            mono(
                name.clone(),
                Type::SMALL,
                if selected { t.text } else { t.text_soft },
            )
            .flex_1()
            .min_w(rems(0.))
            .truncate(),
        )
        .children(row.note().map(|note| {
            div()
                .flex_shrink_0()
                .typeset(Type::MICRO)
                .text_color(t.tone(note.tone))
                .child(note.text.clone())
        }))
        .on_click(cx.listener(move |ws, _, _, cx| {
            ws.pick_plugin(&plugin, entry.as_deref(), cx)
        }))
        .into_any_element()
}

/// The pane: the row's head, its tabs, and the tab shown.
fn pane(
    ws: &Workspace,
    selection: &Selection,
    compact: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let rows = rows(ws);
    let Some(row) = rows.iter().find(|row| row.is(selection)) else {
        return ui::empty("No plugin selected.", t).into_any_element();
    };
    let name = row.name().to_owned();
    let ink = t.roles.plugin(&name).unwrap_or(t.mark(&name));
    let back = compact.then(|| {
        div()
            .id("plugins-back")
            .cursor_pointer()
            .typeset(Type::CAPTION)
            .text_color(t.roles.link)
            .child("‹ Plugins")
            .on_click(cx.listener(|ws, _, _, cx| ws.unpick_plugin(cx)))
    });
    let head = div()
        .flex()
        .flex_col()
        .gap(sp(1.5))
        .child(
            div()
                .flex()
                .items_center()
                .gap(sp(2.5))
                .child(div().size(rems(0.5625)).rounded(radius::FULL).bg(ink))
                .child(mono(name.clone(), Type::TITLE, ink))
                .children(row.note().map(|note| {
                    ui::pill(
                        note.text.clone(),
                        t.tone(note.tone),
                        t.tone(note.tone).opacity(0.12),
                    )
                })),
        )
        .child(
            div()
                .text_color(t.muted)
                .leading(1.5)
                .child(row.description().to_owned()),
        );
    let tabs = div()
        .flex()
        .gap(sp(6.))
        .border_b_1()
        .border_color(t.border)
        .children(Tab::ALL.into_iter().map(|tab| {
            let on = tab == selection.tab;
            div()
                .id(SharedString::from(format!("plugin-tab-{}", tab.label())))
                .cursor_pointer()
                .pb(sp(2.5))
                .typeset(Type::SMALL)
                .text_color(if on { t.text } else { t.dim })
                .when(on, |tab| tab.border_b_2().border_color(t.accent))
                .child(tab.label())
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.set_plugin_tab(tab, cx)),
                )
        }));
    let body = match selection.tab {
        Tab::Settings => settings(ws, row, selection, t, cx),
        Tab::Work => work(ws, row, t, cx),
        Tab::About => about(row, t),
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(5.))
        .children(back)
        .child(head)
        .child(tabs)
        .child(body)
        .into_any_element()
}

/// The settings tab: the scope switch, the plugin's pane for that
/// scope, and what the scope means.
fn settings(
    ws: &Workspace,
    row: &Row<'_>,
    selection: &Selection,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let plugin = row.plugin.name.clone();
    if !row.plugin.settings {
        return ui::empty(format!("{} has nothing to set.", row.name()), t)
            .into_any_element();
    }
    // A repository no longer listed shows everywhere.
    let scope = selection
        .scope
        .clone()
        .filter(|repo| ws.catalog.repo(repo).is_some());
    let drawn = ws.plugin_settings_pane(
        &plugin,
        scope.as_deref(),
        row.entry.map(|entry| entry.name.as_str()),
        cx,
    );
    div()
        .flex()
        .flex_col()
        .gap(sp(4.))
        .child(scope_switch(
            ws,
            &plugin,
            scope.as_deref(),
            t,
            cx,
            |ws, scope, cx| ws.set_plugin_scope(scope, cx),
        ))
        .children(drawn)
        .child(scope_line(ws, &plugin, scope.as_deref(), t, cx))
        .into_any_element()
}

/// Everywhere and each repository, as a segmented switch; a repository
/// with its own copy of `plugin`'s settings is marked.
pub fn scope_switch(
    ws: &Workspace,
    plugin: &str,
    scope: Option<&str>,
    t: &Theme,
    cx: &mut Context<Workspace>,
    pick: impl Fn(&mut Workspace, Option<String>, &mut Context<Workspace>)
    + Clone
    + 'static,
) -> AnyElement {
    let options = std::iter::once(None)
        .chain(ws.catalog.repos.iter().map(|repo| Some(repo.name.clone())));
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(2.5))
        .child(
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("Applies to"),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap(sp(0.5))
                .p(sp(0.75))
                .rounded(radius::CONTROL)
                .well(t)
                .children(options.map(|option| {
                    let on = option.as_deref() == scope;
                    let own = option.as_deref().is_some_and(|repo| {
                        ws.catalog.has_own_settings(repo, plugin)
                    });
                    let label =
                        option.clone().unwrap_or_else(|| "Everywhere".into());
                    let pick = pick.clone();
                    div()
                        .id(SharedString::from(format!("scope-{label}")))
                        .flex()
                        .items_center()
                        .gap(sp(1.5))
                        .px(sp(3.))
                        .py(sp(1.5))
                        .rounded(radius::SMALL)
                        .cursor_pointer()
                        .typeset(Type::CAPTION)
                        .text_color(if on { t.text } else { t.dim })
                        .when(on, |option| option.key(t))
                        .when(own, |option| {
                            option.child(
                                div()
                                    .size(rems(0.375))
                                    .rounded(radius::FULL)
                                    .bg(t.roles.live),
                            )
                        })
                        .child(label)
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            pick(ws, option.clone(), cx)
                        }))
                })),
        )
        .into_any_element()
}

/// What the scope shown means, and the way back to the value
/// everywhere for a repository with its own copy.
pub fn scope_line(
    ws: &Workspace,
    plugin: &str,
    scope: Option<&str>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let line = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(2.))
        .typeset(Type::CAPTION)
        .text_color(t.dim);
    let line = match scope {
        None => line.child("Every repository without its own copy uses these."),
        Some(repo) if ws.catalog.has_own_settings(repo, plugin) => {
            let (plugin, repo) = (plugin.to_owned(), repo.to_owned());
            line.child(format!("{repo} keeps its own copy.")).child(
                div()
                    .id("drop-own-settings")
                    .cursor_pointer()
                    .text_color(t.roles.link)
                    .child("Use the value everywhere")
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        if let Some(erased) =
                            crate::plugins::registry().get(&plugin)
                        {
                            ws.plugin_handle(erased.name())
                                .drop_settings(&repo, cx);
                        }
                    })),
            )
        }
        Some(repo) => line.child(format!(
            "{repo} uses the value everywhere until you change something here."
        )),
    };
    line.child(
        div()
            .ml_auto()
            .child("Applies from each chat's next message."),
    )
    .into_any_element()
}

/// What it did: the plugin's page, drawn in the pane.
fn work(
    ws: &Workspace,
    row: &Row<'_>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let Some(Route::Plugin {
        plugin,
        page,
        mut params,
    }) = ws.plugin_route(row.plugin)
    else {
        let text = if row.plugin.page.is_some() {
            "Open a repository to see what it did there."
        } else {
            "It keeps no page of what it did."
        };
        return ui::empty(text, t).into_any_element();
    };
    if let Some(entry) = row.entry {
        params.insert(tau_ui_plugin::ENTRY.to_owned(), entry.name.clone());
    }
    ws.plugin_page(&plugin, &page, &params, cx)
        .unwrap_or_else(|| {
            ui::empty("Nothing to show yet.", t).into_any_element()
        })
}

/// How it works: what it is for, where it steps into a run, and what it
/// cost.
fn about(row: &Row<'_>, t: &Theme) -> AnyElement {
    let seams = row.seams();
    div()
        .flex()
        .flex_col()
        .gap(sp(4.))
        .child(
            div()
                .text_color(t.text_soft)
                .leading(1.5)
                .child(row.description().to_owned()),
        )
        .child(ui::heading("Where it steps in", t))
        .child(if seams.is_empty() {
            div()
                .typeset(Type::CAPTION)
                .text_color(t.dim)
                .child("Only in the interface.")
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap(sp(1.5))
                .children(seams.iter().map(|seam| {
                    ui::tag(seam_label(*seam), Type::CAPTION, t.text_soft, t)
                }))
        })
        .when(row.entry.is_none(), |about| {
            about.child(
                div()
                    .typeset(Type::CAPTION)
                    .text_color(t.dim)
                    .child(format!(
                        "{} charged lately.",
                        usd(row.plugin.spend)
                    )),
            )
        })
        .into_any_element()
}

fn seam_label(seam: Seam) -> &'static str {
    match seam {
        Seam::Start => "as a run starts",
        Seam::Tools => "tools it adds",
        Seam::BeforeTool => "before each call",
        Seam::AfterTool => "after each call",
        Seam::Rewrite => "rewrites the context",
        Seam::BeforeStop => "before a run stops",
        Seam::Finish => "as a run ends",
    }
}
