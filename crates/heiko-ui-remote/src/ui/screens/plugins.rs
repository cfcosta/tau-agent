//! The plugins built in (ADR 0029): a list grouped by what each plugin
//! is for, and a pane for the one selected, with its settings, what it
//! did and how it works. On a phone the list and the pane are two steps.
//! The person's own plugins have their own page (ADR 0036), which the
//! list links to.

use gpui::{AnyElement, Context, SharedString, div, prelude::*, rems};
use heiko_ui_plugin::{CatalogEntry, Group, Note, Seam, StoreReach};

use crate::{
    catalog::PluginInfo,
    route::Route,
    theme::{Design as _, Theme, Type, sp, weight},
    ui::{self, Material as _, mono},
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
        self.entry
            .map_or(&self.plugin.reach.seams, |entry| &entry.seams)
    }

    fn is(&self, selection: &Selection) -> bool {
        selection.plugin.as_deref() == Some(self.plugin.name.as_str())
            && selection.entry.as_deref() == self.entry.map(|e| e.name.as_str())
    }
}

/// Every row, in the catalog's order: each plugin built in, then its
/// entries. The person's own plugins have a page of their own.
fn rows(ws: &Workspace) -> Vec<Row<'_>> {
    ws.catalog
        .built_in()
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
        // The first row the list shows.
        let rows = rows(self);
        Group::ALL.into_iter().find_map(|group| {
            rows.iter().find(|row| row.group() == group).map(|row| {
                (
                    row.plugin.name.clone(),
                    row.entry.map(|entry| entry.name.clone()),
                )
            })
        })
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
                .pl(sp(8.))
                .pr(sp(4.))
                .py(sp(6.))
                .child(list(ws, &selection, t, cx)),
        )
        .child(
            div()
                .id("plugins-pane")
                .flex_1()
                .min_w(rems(0.))
                .overflow_y_scroll()
                .px(sp(8.))
                .py(sp(6.))
                .child(
                    div()
                        .max_w(rems(52.))
                        .child(pane(ws, &selection, false, t, cx)),
                ),
        )
        .into_any_element()
}

/// A group's name over its rows.
fn group_label(text: impl Into<SharedString>, t: &Theme) -> gpui::Div {
    div()
        .pb(sp(1.5))
        .typeset(Type::SMALL)
        .font_weight(weight::EMPHASIS)
        .text_color(t.muted)
        .child(text.into())
}

/// The list: its title with how many plugins are built in and the way
/// to the person's own, then each group's rows.
fn list(
    ws: &Workspace,
    selection: &Selection,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let rows = rows(ws);
    let roles = roles(ws, t, cx);
    // The person's own plugins are on a page of their own.
    let yours = ws.yours_route(None).map(|route| {
        div()
            .id("plugins-yours-link")
            .child(ui::link("your own ›", t))
            .on_click(
                cx.listener(move |ws, _, _, cx| ws.navigate(route.clone(), cx)),
            )
    });
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
                .child(group_label(group.label(), t))
                .children(rows)
        })
    });
    div()
        .flex()
        .flex_col()
        .gap(sp(4.5))
        .child(
            ui::section_header(
                "Built in",
                Some(ws.catalog.built_in().count().to_string().into()),
                t,
            )
            .children(yours),
        )
        // Choices first: who fills a slot several plugins can.
        .children(roles)
        .children(groups)
        .into_any_element()
}

/// The slots several plugins can fill (ADR 0045): each with those
/// plugins in the slot's order, the one that fills it marked. Picking
/// another is the person's choice, kept in the UI state, so phones show
/// it and it outlives a restart.
fn roles(
    ws: &Workspace,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> Option<AnyElement> {
    let contested: Vec<_> = ws
        .catalog
        .slots
        .iter()
        .filter(|slot| slot.candidates.len() > 1)
        .collect();
    if contested.is_empty() {
        return None;
    }
    let slots = contested.into_iter().map(|slot| {
        let filler = ws.slot_filler(&slot.name).map(str::to_owned);
        let rows = slot.candidates.iter().map(|plugin| {
            let on = filler.as_deref() == Some(plugin.as_str());
            let id = format!("slot:{}:{plugin}", slot.name);
            let (name, picked) = (slot.name.clone(), plugin.clone());
            div()
                .id(SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .flex()
                .items_center()
                .gap(sp(2.5))
                .min_h(rems(1.75))
                .cursor_pointer()
                .child(
                    div()
                        .size(rems(0.5))
                        .flex_shrink_0()
                        .border_1()
                        .border_color(if on { t.accent } else { t.slate })
                        .when(on, |square| square.bg(t.accent)),
                )
                .child(
                    mono(
                        plugin.clone(),
                        Type::SMALL,
                        if on { t.bright } else { t.text_soft },
                    )
                    .when(on, |name| name.font_weight(weight::EMPHASIS)),
                )
                .on_click(cx.listener(move |ws, _, _, cx| {
                    ws.pick_slot(&name, &picked, cx)
                }))
        });
        div()
            .flex()
            .flex_col()
            .child(mono(slot.name.clone(), Type::CAPTION, t.dim).pb(sp(1.)))
            .children(rows)
    });
    Some(
        div()
            .flex()
            .flex_col()
            .gap(sp(3.))
            .child(group_label("Roles", t))
            .children(slots)
            .into_any_element(),
    )
}

/// A row of the list: the name in mono, and its note. The selected one
/// is the offset block.
fn list_row(
    row: &Row<'_>,
    selected: bool,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let name = row.name().to_owned();
    let plugin = row.plugin.name.clone();
    let entry = row.entry.map(|entry| entry.name.clone());
    div()
        .id(SharedString::from(format!("plugin-row-{plugin}-{name}")))
        .flex()
        .items_center()
        .gap(sp(2.5))
        .min_h(rems(1.6875))
        .cursor_pointer()
        .when(selected, |row| {
            row.mx(sp(-2.))
                .px(sp(2.))
                .printed(3., t)
                .font_weight(weight::STRONG)
        })
        .when(row.entry.is_some(), |row| row.pl(sp(4.)))
        .child(
            mono(
                name.clone(),
                Type::SMALL,
                if selected {
                    t.roles.on_primary
                } else {
                    t.text_soft
                },
            )
            .flex_1()
            .min_w(rems(0.))
            .truncate(),
        )
        .children(row.note().map(|note| {
            mono(
                note.text.clone(),
                Type::MICRO,
                if selected {
                    t.roles.on_primary
                } else {
                    t.tone(note.tone)
                },
            )
            .flex_shrink_0()
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
    let back = compact.then(|| {
        div()
            .id("plugins-back")
            .flex()
            .child(ui::link("‹ Plugins", t))
            .on_click(cx.listener(|ws, _, _, cx| ws.unpick_plugin(cx)))
    });
    // The name as the page's title, with its note and what it is for
    // beside it, over the heavy rule.
    let head = div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(sp(3.))
        .pb(sp(2.5))
        .border_b_2()
        .border_color(t.rule)
        .child(
            div()
                .typeset(Type::PAGE_TITLE)
                .text_color(t.bright)
                .child(row.name().to_owned()),
        )
        .children(row.note().map(|note| {
            mono(note.text.clone(), Type::CAPTION, t.tone(note.tone))
        }))
        .child(
            div()
                .flex_1()
                .min_w(rems(12.))
                .text_right()
                .typeset(Type::CAPTION)
                .text_color(t.muted)
                .child(row.description().to_owned()),
        );
    // The tabs it has: settings when it draws them, its work when it
    // keeps a page; the one picked, else the first.
    let has = |tab: Tab| match tab {
        Tab::Settings => row.plugin.settings,
        Tab::Work => row.plugin.page.is_some(),
        Tab::About => true,
    };
    let shown = if has(selection.tab) {
        selection.tab
    } else {
        Tab::ALL
            .into_iter()
            .find(|tab| has(*tab))
            .unwrap_or(Tab::About)
    };
    // Underlined words; the one shown is brighter, over a blue line.
    let tabs = div()
        .flex()
        .gap(sp(5.))
        .pb(sp(3.))
        .border_b_1()
        .border_color(t.hairline)
        .children(Tab::ALL.into_iter().filter(|tab| has(*tab)).map(|tab| {
            let on = tab == shown;
            div()
                .id(SharedString::from(format!("plugin-tab-{}", tab.label())))
                .cursor_pointer()
                .border_b_1()
                .border_color(if on { t.blue } else { t.slate })
                .typeset(Type::BODY)
                .text_color(if on { t.bright } else { t.text_soft })
                .when(on, |tab| tab.font_weight(weight::EMPHASIS))
                .child(tab.label())
                .on_click(
                    cx.listener(move |ws, _, _, cx| ws.set_plugin_tab(tab, cx)),
                )
        }));
    let body = match shown {
        Tab::Settings => settings(ws, row, selection, t, cx),
        Tab::Work => work(ws, row, t, cx),
        Tab::About => about(row, t),
    };
    div()
        .flex()
        .flex_col()
        .gap(sp(4.))
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

/// "Applies to", then everywhere and each repository as square
/// segments, the one shown outlined; a repository with its own copy of
/// `plugin`'s settings is marked.
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
        .chain(ws.catalog.listed().map(|repo| Some(repo.name.clone())));
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(sp(2.5))
        .pb(sp(4.))
        .border_b_1()
        .border_color(t.hairline)
        .child(
            div()
                .flex_1()
                .typeset(Type::BODY)
                .text_color(t.text)
                .child("Applies to"),
        )
        .child(div().flex().flex_wrap().gap(sp(1.)).children(options.map(
            |option| {
                let on = option.as_deref() == scope;
                let own = option.as_deref().is_some_and(|repo| {
                    ws.catalog.has_own_settings(repo, plugin)
                });
                let label =
                    option.clone().unwrap_or_else(|| "everywhere".into());
                let pick = pick.clone();
                div()
                    .id(SharedString::from(format!("scope-{label}")))
                    .flex()
                    .items_center()
                    .gap(sp(1.5))
                    .px(sp(2.5))
                    .py(sp(1.))
                    .border_1()
                    .cursor_pointer()
                    .when(on, |option| {
                        option
                            .border_color(t.bright)
                            .font_weight(weight::EMPHASIS)
                            .lifted(t)
                    })
                    .when(!on, |option| {
                        option.border_color(t.raised).bg(t.raised)
                    })
                    .when(own, |option| {
                        option.child(
                            div()
                                .size(rems(0.375))
                                .flex_shrink_0()
                                .bg(t.roles.live),
                        )
                    })
                    .child(mono(
                        label,
                        Type::CAPTION,
                        if on { t.bright } else { t.dim },
                    ))
                    .on_click(cx.listener(move |ws, _, _, cx| {
                        pick(ws, option.clone(), cx)
                    }))
            },
        )))
        .into_any_element()
}

/// Under the settings: the way back to the value everywhere for a
/// repository with its own copy, and when a change applies.
pub fn scope_line(
    ws: &Workspace,
    plugin: &str,
    scope: Option<&str>,
    t: &Theme,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let own = scope
        .filter(|repo| ws.catalog.has_own_settings(repo, plugin))
        .map(|repo| {
            let (plugin, repo) = (plugin.to_owned(), repo.to_owned());
            div()
                .flex()
                .items_baseline()
                .gap(sp(2.))
                .child(format!("{repo} keeps its own copy"))
                .child(
                    div()
                        .id("drop-own-settings")
                        .child(ui::link("use the value everywhere", t))
                        .on_click(cx.listener(move |ws, _, _, cx| {
                            if let Some(erased) =
                                crate::plugins::registry().get(&plugin)
                            {
                                ws.plugin_handle(erased.name())
                                    .drop_settings(&repo);
                                ws.drain_plugin_requests(cx);
                            }
                        })),
                )
        });
    div()
        .flex()
        .flex_wrap()
        .items_baseline()
        .gap(sp(2.))
        .typeset(Type::CAPTION)
        .text_color(t.dim)
        .children(own)
        .child(div().ml_auto().child("from each chat's next message"))
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
        params.insert(heiko_ui_plugin::ENTRY.to_owned(), entry.name.clone());
    }
    ws.plugin_page(&plugin, &page, &params, cx)
        .unwrap_or_else(|| {
            ui::empty("Nothing to show yet.", t).into_any_element()
        })
}

/// How it works: what it is for, and where it steps into a run.
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
        .when_some(reach(row, t), |about, reach| about.child(reach))
        .into_any_element()
}

/// What the plugin declares it reaches beyond its seams, when it
/// declares any (`Declaration`): the slots it fills, the services it
/// provides and uses, the tools it calls, the store beyond its own
/// records, the network, secrets, and paths outside repositories.
fn reach(row: &Row<'_>, t: &Theme) -> Option<AnyElement> {
    if row.entry.is_some() {
        return None;
    }
    let reach = &row.plugin.reach;
    let store = if reach.store == StoreReach::Read {
        vec!["every run's records".to_owned()]
    } else {
        Vec::new()
    };
    let groups: Vec<(&str, &[String])> = vec![
        ("Slots it fills", &reach.fills),
        ("Services it gives", &reach.provides),
        ("Services it uses", &reach.services),
        ("Tools it calls", &reach.tools),
        ("The store it reads", &store),
        ("Where it connects", &reach.network),
        ("Secrets it keeps", &reach.secrets),
        ("What it reads outside repositories", &reach.reads),
    ];
    let groups: Vec<_> = groups
        .into_iter()
        .filter(|(_, names)| !names.is_empty())
        .collect();
    if groups.is_empty() {
        return None;
    }
    Some(
        div()
            .id("plugin-reach")
            .debug_selector(|| "plugin-reach".to_owned())
            .flex()
            .flex_col()
            .gap(sp(2.))
            .children(groups.into_iter().map(|(heading, names)| {
                div()
                    .flex()
                    .flex_col()
                    .gap(sp(1.5))
                    .child(ui::heading(heading, t))
                    .child(div().flex().flex_wrap().gap(sp(1.5)).children(
                        names.iter().map(|name| {
                            ui::tag(name.clone(), Type::CAPTION, t.text_soft, t)
                        }),
                    ))
            }))
            .child(div().typeset(Type::CAPTION).text_color(t.dim).child(
                "What the plugin declares. heiko calls only its seams, gives \
                 it only its services and takes only its slots; the rest \
                 is a statement, not a limit, for a plugin built into it.",
            ))
            .into_any_element(),
    )
}

fn seam_label(seam: Seam) -> &'static str {
    match seam {
        Seam::Start => "as a run starts",
        Seam::Tools => "tools it adds",
        Seam::StartIdle => "compacting an idle run",
        Seam::BeforeRequest => "before each request",
        Seam::BeforeTool => "before each call",
        Seam::AfterTool => "after each call",
        Seam::OnEvent => "every run event",
        Seam::Rewrite => "rewrites the context",
        Seam::Rewritten => "after a rewrite",
        Seam::BeforeStop => "before a run stops",
        Seam::Finish => "as a run ends",
    }
}
